//! The sections of a video that the user wants to keep.

use crate::time::Ticks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeRange {
    pub start: Ticks,
    pub end: Ticks,
}

impl TimeRange {
    pub fn duration(self) -> Ticks {
        Ticks(self.end.0 - self.start.0)
    }
}

/// Sections to keep, in time order and never overlapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutList {
    duration: Ticks,
    ranges: Vec<TimeRange>,
    pending_in: Option<Ticks>,
}

impl CutList {
    pub fn new(duration: Ticks) -> Self {
        Self {
            duration,
            ranges: Vec::new(),
            pending_in: None,
        }
    }

    pub fn ranges(&self) -> &[TimeRange] {
        &self.ranges
    }

    pub fn pending_in(&self) -> Option<Ticks> {
        self.pending_in
    }

    /// Starts a new section at `time`.
    pub fn mark_in(&mut self, time: Ticks) {
        self.pending_in = Some(self.clamp(time));
    }

    /// Ends a section at `time`, which is left out of the section. Without a
    /// pending start, the section begins where the previous one ended. Returns
    /// false if the section would be empty, leaving the list unchanged.
    pub fn mark_out(&mut self, time: Ticks) -> bool {
        let end = self.clamp(time);
        let start = self.pending_in.unwrap_or_else(|| {
            self.ranges
                .iter()
                .rev()
                .map(|range| range.end)
                .find(|previous_end| *previous_end < end)
                .unwrap_or_default()
        });
        if start >= end {
            return false;
        }
        self.pending_in = None;
        self.insert(TimeRange { start, end });
        true
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.ranges.len() {
            self.ranges.remove(index);
        }
    }

    pub fn clear(&mut self) {
        self.ranges.clear();
        self.pending_in = None;
    }

    /// Moves one edge of a section, stopping short of its other edge and of
    /// neighbouring sections.
    pub fn move_edge(&mut self, index: usize, edge: RangeEdge, time: Ticks) {
        let Some(range) = self.ranges.get(index).copied() else {
            return;
        };
        let time = self.clamp(time);
        match edge {
            RangeEdge::Start => {
                let lower = index
                    .checked_sub(1)
                    .map_or(Ticks(0), |previous| self.ranges[previous].end);
                self.ranges[index].start = time.clamp(lower, Ticks(range.end.0 - 1));
            }
            RangeEdge::End => {
                let upper = self
                    .ranges
                    .get(index + 1)
                    .map_or(self.duration, |next| next.start);
                self.ranges[index].end = time.clamp(Ticks(range.start.0 + 1), upper);
            }
        }
    }

    /// The sections to export. A start with no end runs to the end of the video.
    /// With nothing marked this is empty, because keeping everything is not a cut.
    pub fn export_ranges(&self) -> Vec<TimeRange> {
        let mut list = self.clone();
        if let Some(start) = list.pending_in.take()
            && start < list.duration
        {
            list.insert(TimeRange {
                start,
                end: list.duration,
            });
        }
        list.ranges
    }

    /// How long the exported video would be.
    pub fn kept_duration(&self) -> Ticks {
        Ticks(
            self.export_ranges()
                .iter()
                .map(|range| range.duration().0)
                .sum(),
        )
    }

    fn clamp(&self, time: Ticks) -> Ticks {
        time.clamp(Ticks(0), self.duration)
    }

    fn insert(&mut self, range: TimeRange) {
        self.ranges.push(range);
        self.ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<TimeRange> = Vec::with_capacity(self.ranges.len());
        for range in self.ranges.drain(..) {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        self.ranges = merged;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeEdge {
    Start,
    End,
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn range(start: i64, end: i64) -> TimeRange {
        TimeRange {
            start: Ticks(start),
            end: Ticks(end),
        }
    }

    fn list() -> CutList {
        CutList::new(Ticks(100))
    }

    #[test]
    fn in_and_out_make_a_section() {
        let mut cuts = list();
        cuts.mark_in(Ticks(10));
        assert!(cuts.mark_out(Ticks(20)));

        assert_eq!(cuts.ranges(), [range(10, 20)]);
        assert_eq!(cuts.pending_in(), None);
    }

    #[test]
    fn out_without_in_starts_at_the_beginning() {
        let mut cuts = list();
        assert!(cuts.mark_out(Ticks(30)));

        assert_eq!(cuts.ranges(), [range(0, 30)]);
    }

    #[test]
    fn out_without_in_continues_from_the_previous_section() {
        let mut cuts = list();
        cuts.mark_in(Ticks(10));
        cuts.mark_out(Ticks(20));
        cuts.mark_out(Ticks(40));

        assert_eq!(cuts.ranges(), [range(10, 40)]);
    }

    #[test]
    fn an_empty_or_backwards_section_is_refused() {
        let mut cuts = list();
        cuts.mark_in(Ticks(50));

        assert!(!cuts.mark_out(Ticks(50)));
        assert!(!cuts.mark_out(Ticks(40)));
        assert!(cuts.ranges().is_empty());
        assert_eq!(cuts.pending_in(), Some(Ticks(50)));
    }

    #[test]
    fn marks_stay_inside_the_video() {
        let mut cuts = list();
        cuts.mark_in(Ticks(-5));
        cuts.mark_out(Ticks(500));

        assert_eq!(cuts.ranges(), [range(0, 100)]);
    }

    #[test]
    fn sections_are_sorted_and_overlaps_merge() {
        let mut cuts = list();
        cuts.mark_in(Ticks(60));
        cuts.mark_out(Ticks(70));
        cuts.mark_in(Ticks(10));
        cuts.mark_out(Ticks(20));
        assert_eq!(cuts.ranges(), [range(10, 20), range(60, 70)]);

        cuts.mark_in(Ticks(15));
        cuts.mark_out(Ticks(65));
        assert_eq!(cuts.ranges(), [range(10, 70)]);
    }

    #[test]
    fn touching_sections_merge() {
        let mut cuts = list();
        cuts.mark_in(Ticks(10));
        cuts.mark_out(Ticks(20));
        cuts.mark_in(Ticks(20));
        cuts.mark_out(Ticks(30));

        assert_eq!(cuts.ranges(), [range(10, 30)]);
    }

    #[test]
    fn export_runs_a_pending_start_to_the_end() {
        let mut cuts = list();
        cuts.mark_in(Ticks(80));

        assert_eq!(cuts.export_ranges(), [range(80, 100)]);
        assert_eq!(cuts.kept_duration(), Ticks(20));
    }

    #[test]
    fn a_pending_start_at_the_very_end_exports_nothing() {
        let mut cuts = list();
        cuts.mark_in(Ticks(100));

        assert!(cuts.export_ranges().is_empty());
    }

    #[test]
    fn nothing_marked_exports_nothing() {
        assert!(list().export_ranges().is_empty());
        assert_eq!(list().kept_duration(), Ticks(0));
    }

    #[test]
    fn edges_stop_at_their_neighbours() {
        let mut cuts = list();
        cuts.mark_in(Ticks(10));
        cuts.mark_out(Ticks(20));
        cuts.mark_in(Ticks(40));
        cuts.mark_out(Ticks(50));

        cuts.move_edge(1, RangeEdge::Start, Ticks(5));
        assert_eq!(cuts.ranges()[1], range(20, 50));

        cuts.move_edge(0, RangeEdge::End, Ticks(90));
        assert_eq!(cuts.ranges()[0], range(10, 20));

        cuts.move_edge(1, RangeEdge::End, Ticks(95));
        assert_eq!(cuts.ranges()[1], range(20, 95));
    }

    #[test]
    fn edges_cannot_cross_each_other() {
        let mut cuts = list();
        cuts.mark_in(Ticks(10));
        cuts.mark_out(Ticks(20));

        cuts.move_edge(0, RangeEdge::Start, Ticks(50));
        assert_eq!(cuts.ranges()[0], range(19, 20));

        cuts.move_edge(0, RangeEdge::End, Ticks(0));
        assert_eq!(cuts.ranges()[0], range(19, 20));
    }

    #[test]
    fn removing_a_section_leaves_the_others() {
        let mut cuts = list();
        cuts.mark_in(Ticks(10));
        cuts.mark_out(Ticks(20));
        cuts.mark_in(Ticks(40));
        cuts.mark_out(Ticks(50));

        cuts.remove(0);
        cuts.remove(9);
        assert_eq!(cuts.ranges(), [range(40, 50)]);
    }
}
