//! Works out how to cut compressed video with as little re-encoding as possible.
//!
//! Frames depend on earlier and sometimes later frames, so a copy can only begin
//! at a point where nothing before it is needed by anything after it. Sections
//! between two such points are copied untouched. Anything else has to be decoded
//! and encoded again.

/// One compressed video frame, in the order it is stored in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameEntry {
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    pub keyframe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutMode {
    /// Cuts land exactly where asked. The short sections around each cut are re-encoded.
    FrameExact,
    /// Nothing is re-encoded. Sections grow outward to the nearest clean points.
    Keyframe,
}

/// A part of one output section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Copy stored frames `first..end` unchanged.
    Copy { first: usize, end: usize },
    /// Decode stored frames `decode_first..decode_end`, then encode the ones
    /// shown from `pts_start` up to but not including `pts_end`.
    Encode {
        decode_first: usize,
        decode_end: usize,
        pts_start: i64,
        pts_end: i64,
    },
}

/// One continuous section of the output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Display time of the section's first frame in the source.
    pub pts_start: i64,
    /// Display time just after the section's last frame in the source.
    pub pts_end: i64,
    pub pieces: Vec<Piece>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutPlan {
    pub sections: Vec<Section>,
    pub copied_frames: usize,
    pub encoded_frames: usize,
}

/// The video's frames, with the positions that are safe to cut at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameIndex {
    frames: Vec<FrameEntry>,
    /// Stored frame positions, sorted by display time.
    display_order: Vec<usize>,
    /// Whether a copy can start at this stored frame.
    clean_point: Vec<bool>,
}

impl FrameIndex {
    pub fn new(frames: Vec<FrameEntry>) -> Self {
        let mut display_order: Vec<usize> = (0..frames.len()).collect();
        display_order.sort_by_key(|&index| frames[index].pts);

        // A clean point is a keyframe where everything stored before it is shown
        // before it, and everything stored after it is shown after it.
        let mut clean_point = vec![false; frames.len()];
        let mut latest_before = i64::MIN;
        let mut earliest_after = vec![i64::MAX; frames.len() + 1];
        for index in (0..frames.len()).rev() {
            earliest_after[index] = earliest_after[index + 1].min(frames[index].pts);
        }
        for (index, frame) in frames.iter().enumerate() {
            clean_point[index] = frame.keyframe
                && latest_before < frame.pts
                && earliest_after[index + 1] > frame.pts;
            latest_before = latest_before.max(frame.pts);
        }

        Self {
            frames,
            display_order,
            clean_point,
        }
    }

    pub fn frames(&self) -> &[FrameEntry] {
        &self.frames
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Display time just after the last frame.
    pub fn end_pts(&self) -> i64 {
        self.display_order.last().map_or(0, |&index| {
            self.frames[index].pts + self.frames[index].duration.max(1)
        })
    }

    /// Display time of the first frame.
    pub fn start_pts(&self) -> i64 {
        self.display_order
            .first()
            .map_or(0, |&index| self.frames[index].pts)
    }

    /// The display time of the frame on screen at `pts`.
    pub fn frame_at(&self, pts: i64) -> Option<i64> {
        let position = self
            .display_order
            .partition_point(|&index| self.frames[index].pts <= pts);
        let index = self.display_order[position.saturating_sub(1)..].first()?;
        Some(self.frames[*index].pts)
    }

    /// The display time of the frame after the one on screen at `pts`.
    pub fn next_frame_after(&self, pts: i64) -> Option<i64> {
        let position = self
            .display_order
            .partition_point(|&index| self.frames[index].pts <= pts);
        self.display_order
            .get(position)
            .map(|&index| self.frames[index].pts)
    }

    /// The display time of the frame before the one on screen at `pts`.
    pub fn previous_frame_before(&self, pts: i64) -> Option<i64> {
        let position = self
            .display_order
            .partition_point(|&index| self.frames[index].pts < pts);
        position
            .checked_sub(1)
            .map(|before| self.frames[self.display_order[before]].pts)
    }

    /// Display times of all clean points.
    pub fn clean_points(&self) -> Vec<i64> {
        self.frames
            .iter()
            .zip(&self.clean_point)
            .filter(|(_, clean)| **clean)
            .map(|(frame, _)| frame.pts)
            .collect()
    }

    /// Plans the cuts for sections given as display time ranges, end excluded.
    pub fn plan(&self, ranges: &[(i64, i64)], mode: CutMode) -> CutPlan {
        let mut sections = match mode {
            CutMode::FrameExact => ranges
                .iter()
                .filter_map(|&range| self.plan_exact(range))
                .collect(),
            CutMode::Keyframe => self.plan_keyframe(ranges),
        };
        sections.retain(|section: &Section| !section.pieces.is_empty());

        let mut plan = CutPlan {
            sections,
            copied_frames: 0,
            encoded_frames: 0,
        };
        for piece in plan.sections.iter().flat_map(|section| &section.pieces) {
            match *piece {
                Piece::Copy { first, end } => plan.copied_frames += end - first,
                Piece::Encode {
                    pts_start, pts_end, ..
                } => plan.encoded_frames += self.count_shown(pts_start, pts_end),
            }
        }
        plan
    }

    fn count_shown(&self, pts_start: i64, pts_end: i64) -> usize {
        let first = self
            .display_order
            .partition_point(|&index| self.frames[index].pts < pts_start);
        let end = self
            .display_order
            .partition_point(|&index| self.frames[index].pts < pts_end);
        end - first
    }

    fn plan_exact(&self, (start, end): (i64, i64)) -> Option<Section> {
        let section_start = self.frame_at(start)?;
        let reaches_end = end >= self.end_pts();
        // A mark can fall partway through a frame. Ending on the next frame's start
        // keeps the same frames, but gives the last one its full time on screen.
        let section_end = if reaches_end {
            self.end_pts()
        } else {
            let first_at_or_after = self
                .display_order
                .partition_point(|&index| self.frames[index].pts < end);
            self.display_order
                .get(first_at_or_after)
                .map_or(self.end_pts(), |&index| self.frames[index].pts)
        };
        if self.count_shown(section_start, section_end) == 0 {
            return None;
        }

        // The stored position of the first clean point at or after each edge.
        let copy_first = self.first_clean_at_or_after(section_start);
        // The last clean point that is not after the end of the section. Reaching
        // the end of the file means the copy can run all the way there.
        let copy_end = if reaches_end {
            Some(self.frames.len())
        } else {
            self.last_clean_at_or_before(section_end)
        };

        let mut pieces = Vec::new();
        match (copy_first, copy_end) {
            (Some(first), Some(copy_end))
                if first < copy_end && self.frames[first].pts < section_end =>
            {
                let copy_start_pts = self.frames[first].pts;
                if section_start < copy_start_pts {
                    pieces.push(self.encode_piece(section_start, copy_start_pts));
                }
                pieces.push(Piece::Copy {
                    first,
                    end: copy_end,
                });
                if !reaches_end && self.frames[copy_end].pts < section_end {
                    pieces.push(self.encode_piece(self.frames[copy_end].pts, section_end));
                }
            }
            _ => pieces.push(self.encode_piece(section_start, section_end)),
        }

        Some(Section {
            pts_start: section_start,
            pts_end: section_end,
            pieces,
        })
    }

    fn plan_keyframe(&self, ranges: &[(i64, i64)]) -> Vec<Section> {
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for &(start, end) in ranges {
            let Some(section_start) = self.frame_at(start) else {
                continue;
            };
            let first = self
                .last_clean_at_or_before(section_start)
                .or_else(|| self.first_clean_at_or_after(section_start));
            let Some(first) = first else {
                continue;
            };
            let last = if end >= self.end_pts() {
                self.frames.len()
            } else {
                self.first_clean_at_or_after(end)
                    .unwrap_or(self.frames.len())
            };
            if first >= last {
                continue;
            }
            // Growing outward can make two sections overlap.
            match spans.last_mut() {
                Some(previous) if first <= previous.1 => previous.1 = previous.1.max(last),
                _ => spans.push((first, last)),
            }
        }

        spans
            .into_iter()
            .map(|(first, end)| Section {
                pts_start: self.frames[first].pts,
                pts_end: if end == self.frames.len() {
                    self.end_pts()
                } else {
                    self.frames[end].pts
                },
                pieces: vec![Piece::Copy { first, end }],
            })
            .collect()
    }

    fn encode_piece(&self, pts_start: i64, pts_end: i64) -> Piece {
        // The nearest keyframe before the piece is where decoding has to begin.
        // The stored order decides this, because display order can differ from it.
        let start_position = self.stored_position_of(pts_start);
        let decode_first = (0..=start_position)
            .rev()
            .find(|&index| self.frames[index].keyframe)
            .unwrap_or(0);

        // Decoding runs until a clean point past the piece, so that every frame
        // the piece shows has received the frames it depends on.
        let decode_end = (decode_first + 1..self.frames.len())
            .find(|&index| self.clean_point[index] && self.frames[index].pts >= pts_end)
            .unwrap_or(self.frames.len());

        Piece::Encode {
            decode_first,
            decode_end,
            pts_start,
            pts_end,
        }
    }

    fn stored_position_of(&self, pts: i64) -> usize {
        self.frames
            .iter()
            .position(|frame| frame.pts == pts)
            .unwrap_or(0)
    }

    fn first_clean_at_or_after(&self, pts: i64) -> Option<usize> {
        (0..self.frames.len())
            .find(|&index| self.clean_point[index] && self.frames[index].pts >= pts)
    }

    fn last_clean_at_or_before(&self, pts: i64) -> Option<usize> {
        (0..self.frames.len())
            .rev()
            .find(|&index| self.clean_point[index] && self.frames[index].pts <= pts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames shown and stored in the same order, 10 units long, with a keyframe
    /// every `gop` frames.
    fn simple(count: i64, gop: i64) -> FrameIndex {
        FrameIndex::new(
            (0..count)
                .map(|n| FrameEntry {
                    pts: n * 10,
                    dts: n * 10,
                    duration: 10,
                    keyframe: n % gop == 0,
                })
                .collect(),
        )
    }

    /// Stored in the order I P B B P B, I P B B P B, I, as encoders with B-frames
    /// produce. Each keyframe starts a closed group of frames.
    fn with_b_frames() -> FrameIndex {
        let displayed = [0, 30, 10, 20, 50, 40, 60, 90, 70, 80, 110, 100, 120];
        FrameIndex::new(
            displayed
                .iter()
                .enumerate()
                .map(|(position, &pts)| FrameEntry {
                    pts,
                    dts: position as i64 * 10 - 20,
                    duration: 10,
                    keyframe: pts % 60 == 0,
                })
                .collect(),
        )
    }

    fn shown_by(index: &FrameIndex, plan: &CutPlan) -> Vec<i64> {
        let mut shown = Vec::new();
        for section in &plan.sections {
            let mut part = Vec::new();
            for piece in &section.pieces {
                match *piece {
                    Piece::Copy { first, end } => {
                        part.extend(index.frames()[first..end].iter().map(|frame| frame.pts));
                    }
                    Piece::Encode {
                        pts_start, pts_end, ..
                    } => part.extend(
                        index
                            .frames()
                            .iter()
                            .map(|frame| frame.pts)
                            .filter(|pts| (pts_start..pts_end).contains(pts)),
                    ),
                }
            }
            part.sort_unstable();
            shown.extend(part);
        }
        shown
    }

    #[test]
    fn keyframes_with_simple_order_are_clean_points() {
        assert_eq!(simple(10, 5).clean_points(), [0, 50]);
    }

    #[test]
    fn a_non_keyframe_is_never_a_clean_point() {
        assert_eq!(simple(4, 100).clean_points(), [0]);
    }

    #[test]
    fn open_gop_keyframes_are_not_clean_points() {
        // Frame 40 is stored before 20 and 30, which are shown before it and may
        // depend on it, so copying cannot start at 40.
        let frames = [
            (0, true),
            (10, false),
            (40, true),
            (20, false),
            (30, false),
            (50, false),
        ]
        .iter()
        .enumerate()
        .map(|(position, &(pts, keyframe))| FrameEntry {
            pts,
            dts: position as i64 * 10,
            duration: 10,
            keyframe,
        })
        .collect();

        assert_eq!(FrameIndex::new(frames).clean_points(), [0]);
    }

    #[test]
    fn b_frame_streams_find_their_clean_points() {
        assert_eq!(with_b_frames().clean_points(), [0, 60, 120]);
    }

    #[test]
    fn frame_exact_cut_between_keyframes_encodes_both_ends_and_copies_the_middle() {
        let index = simple(30, 10);
        let plan = index.plan(&[(50, 250)], CutMode::FrameExact);

        assert_eq!(
            plan.sections[0].pieces,
            [
                Piece::Encode {
                    decode_first: 0,
                    decode_end: 10,
                    pts_start: 50,
                    pts_end: 100
                },
                Piece::Copy { first: 10, end: 20 },
                Piece::Encode {
                    decode_first: 20,
                    decode_end: 30,
                    pts_start: 200,
                    pts_end: 250
                },
            ]
        );
        assert_eq!(plan.encoded_frames, 10);
        assert_eq!(plan.copied_frames, 10);
    }

    #[test]
    fn frame_exact_cut_on_clean_points_encodes_nothing() {
        let index = simple(30, 10);
        let plan = index.plan(&[(100, 200)], CutMode::FrameExact);

        assert_eq!(
            plan.sections[0].pieces,
            [Piece::Copy { first: 10, end: 20 }]
        );
        assert_eq!(plan.encoded_frames, 0);
    }

    #[test]
    fn frame_exact_cut_inside_one_gop_encodes_only_that_part() {
        let index = simple(30, 10);
        let plan = index.plan(&[(120, 150)], CutMode::FrameExact);

        assert_eq!(
            plan.sections[0].pieces,
            [Piece::Encode {
                decode_first: 10,
                decode_end: 20,
                pts_start: 120,
                pts_end: 150
            }]
        );
        assert_eq!(plan.encoded_frames, 3);
        assert_eq!(plan.copied_frames, 0);
    }

    #[test]
    fn a_cut_reaching_the_end_of_the_video_copies_to_the_end() {
        let index = simple(30, 10);
        let plan = index.plan(&[(200, 300)], CutMode::FrameExact);

        assert_eq!(
            plan.sections[0].pieces,
            [Piece::Copy { first: 20, end: 30 }]
        );
        assert_eq!(plan.sections[0].pts_end, 300);
    }

    #[test]
    fn a_cut_starting_mid_frame_starts_with_the_frame_on_screen() {
        let index = simple(30, 10);
        let plan = index.plan(&[(105, 200)], CutMode::FrameExact);

        assert_eq!(plan.sections[0].pts_start, 100);
    }

    #[test]
    fn a_cut_ending_mid_frame_gives_the_last_frame_its_full_time() {
        let index = simple(30, 10);
        let plan = index.plan(&[(50, 123)], CutMode::FrameExact);

        assert_eq!(plan.sections[0].pts_end, 130);
        assert_eq!(shown_by(&index, &plan).last(), Some(&120));
    }

    #[test]
    fn a_cut_ending_just_before_a_clean_point_needs_no_tail_encode() {
        let index = simple(30, 10);
        let plan = index.plan(&[(100, 195)], CutMode::FrameExact);

        assert_eq!(
            plan.sections[0].pieces,
            [Piece::Copy { first: 10, end: 20 }]
        );
        assert_eq!(plan.sections[0].pts_end, 200);
    }

    #[test]
    fn frame_exact_shows_exactly_the_requested_frames() {
        let index = simple(100, 12);
        for &(start, end) in &[(5, 15), (35, 700), (120, 240), (0, 1000), (995, 1000)] {
            let plan = index.plan(&[(start, end)], CutMode::FrameExact);
            let expected: Vec<i64> = (0..100)
                .map(|n| n * 10)
                .filter(|&pts| pts + 10 > start && pts < end)
                .collect();

            assert_eq!(shown_by(&index, &plan), expected, "range {start}..{end}");
        }
    }

    #[test]
    fn frame_exact_with_b_frames_shows_exactly_the_requested_frames() {
        let index = with_b_frames();
        for &(start, end) in &[
            (0, 130),
            (10, 50),
            (30, 100),
            (60, 120),
            (70, 80),
            (40, 130),
        ] {
            let plan = index.plan(&[(start, end)], CutMode::FrameExact);
            let expected: Vec<i64> = (0..13)
                .map(|n| n * 10)
                .filter(|&pts| pts >= start && pts < end)
                .collect();

            assert_eq!(shown_by(&index, &plan), expected, "range {start}..{end}");
        }
    }

    #[test]
    fn encoding_with_b_frames_decodes_the_frames_it_depends_on() {
        let index = with_b_frames();
        let plan = index.plan(&[(10, 50)], CutMode::FrameExact);

        // Frame 40 is stored at position 5 and depends on frame 50 at position 4,
        // so the decode must run from the keyframe through to the next clean point.
        assert_eq!(
            plan.sections[0].pieces,
            [Piece::Encode {
                decode_first: 0,
                decode_end: 6,
                pts_start: 10,
                pts_end: 50
            }]
        );
    }

    #[test]
    fn keyframe_mode_grows_outward_and_never_encodes() {
        let index = simple(30, 10);
        let plan = index.plan(&[(50, 150)], CutMode::Keyframe);

        assert_eq!(plan.sections[0].pieces, [Piece::Copy { first: 0, end: 20 }]);
        assert_eq!(
            (plan.sections[0].pts_start, plan.sections[0].pts_end),
            (0, 200)
        );
        assert_eq!(plan.encoded_frames, 0);
    }

    #[test]
    fn keyframe_mode_on_clean_points_stays_exact() {
        let index = simple(30, 10);
        let plan = index.plan(&[(100, 200)], CutMode::Keyframe);

        assert_eq!(
            plan.sections[0].pieces,
            [Piece::Copy { first: 10, end: 20 }]
        );
    }

    #[test]
    fn keyframe_mode_merges_sections_that_grow_into_each_other() {
        let index = simple(30, 10);
        let plan = index.plan(&[(10, 30), (50, 70)], CutMode::Keyframe);

        assert_eq!(plan.sections.len(), 1);
        assert_eq!(plan.sections[0].pieces, [Piece::Copy { first: 0, end: 10 }]);
    }

    #[test]
    fn several_sections_are_planned_in_order() {
        let index = simple(30, 10);
        let plan = index.plan(&[(0, 100), (200, 300)], CutMode::FrameExact);

        assert_eq!(plan.sections.len(), 2);
        assert_eq!(plan.encoded_frames, 0);
        assert_eq!(plan.copied_frames, 20);
    }

    #[test]
    fn frame_lookups_follow_display_order() {
        let index = with_b_frames();

        assert_eq!(index.frame_at(35), Some(30));
        assert_eq!(index.frame_at(-5), Some(0));
        assert_eq!(index.frame_at(9_999), Some(120));
        assert_eq!(index.next_frame_after(30), Some(40));
        assert_eq!(index.next_frame_after(35), Some(40));
        assert_eq!(index.next_frame_after(120), None);
        assert_eq!(index.previous_frame_before(30), Some(20));
        assert_eq!(index.previous_frame_before(0), None);
        assert_eq!(index.end_pts(), 130);
    }

    #[test]
    fn variable_frame_rate_cuts_land_on_real_frames() {
        let times = [0, 5, 40, 41, 90, 200, 210];
        let index = FrameIndex::new(
            times
                .windows(2)
                .map(|pair| (pair[0], pair[1] - pair[0]))
                .enumerate()
                .map(|(position, (pts, duration))| FrameEntry {
                    pts,
                    dts: pts,
                    duration,
                    keyframe: position % 3 == 0,
                })
                .collect(),
        );

        let plan = index.plan(&[(45, 150)], CutMode::FrameExact);

        assert_eq!(plan.sections[0].pts_start, 41);
        assert_eq!(shown_by(&index, &plan), [41, 90]);
    }

    #[test]
    fn an_empty_video_plans_nothing() {
        let index = FrameIndex::new(Vec::new());

        assert!(
            index
                .plan(&[(0, 100)], CutMode::FrameExact)
                .sections
                .is_empty()
        );
        assert!(
            index
                .plan(&[(0, 100)], CutMode::Keyframe)
                .sections
                .is_empty()
        );
    }
}
