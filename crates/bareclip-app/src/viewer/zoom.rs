//! How far the player is zoomed into the picture, and which part of it shows.
//!
//! Everything is worked out from the view's rectangle, the video's size in pixels and the
//! zoom, so it can be tested on its own. The zoom is how many points on screen one pixel of
//! the video takes; `None` means fit, which is the whole picture as large as the view allows.

use eframe::egui::{Pos2, Rect, Vec2, pos2};

/// The closest the player zooms, in real pixels on screen per pixel of video.
pub const MOST_ZOOM: f32 = 8.0;
/// How much one notch of the wheel or a press of Ctrl+= zooms.
pub const STEP: f32 = 1.25;
/// The furthest out the player zooms, as a share of fit, leaving the picture a quarter of the
/// size that fits.
pub const LEAST_OF_FIT: f32 = 0.25;

/// The zoom and which part of the video is in the middle of the view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Zoom {
    /// Points on screen per pixel of video when zoomed in, or `None` for fit or zoomed out.
    scale: Option<f32>,
    /// When zoomed out, the share of fit the picture is shown at, so it keeps that share as
    /// the window changes size.
    out: Option<f32>,
    /// The point of the video, in its pixels, at the middle of the view.
    centre: Pos2,
}

/// The scale at which the whole video just fits the view.
pub fn fit_scale(view: Vec2, video: Vec2) -> f32 {
    if video.x <= 0.0 || video.y <= 0.0 {
        return 1.0;
    }
    (view.x / video.x).min(view.y / video.y)
}

impl Zoom {
    pub fn is_fit(&self) -> bool {
        self.scale.is_none() && self.out.is_none()
    }

    pub fn fit() -> Self {
        Self::default()
    }

    /// Points on screen per pixel of video.
    pub fn scale(&self, view: Rect, video: Vec2) -> f32 {
        let fit = fit_scale(view.size(), video);
        match (self.scale, self.out) {
            (Some(scale), _) => scale.max(fit),
            (None, Some(share)) => fit * share,
            (None, None) => fit,
        }
    }

    /// Whether the picture is larger than fit, so it can be panned and a click doesn't play.
    pub fn is_zoomed_in(&self, view: Rect, video: Vec2) -> bool {
        self.scale(view, video) > fit_scale(view.size(), video) * 1.0001
    }

    /// The zoom as a percentage of the video's real pixels, as the bar shows it.
    pub fn percent(&self, view: Rect, video: Vec2, pixels_per_point: f32) -> f32 {
        self.scale(view, video) * pixels_per_point * 100.0
    }

    /// The largest scale in points, which is `MOST_ZOOM` real pixels, or fit if that is larger.
    fn most(view: Rect, video: Vec2, pixels_per_point: f32) -> f32 {
        (MOST_ZOOM / pixels_per_point).max(fit_scale(view.size(), video))
    }

    /// Where the whole video is drawn on screen. Zoomed in, it is larger than the view and held
    /// so it always covers it; zoomed out, it is smaller and sits in the middle.
    pub fn picture_rect(&self, view: Rect, video: Vec2) -> Rect {
        let scale = self.scale(view, video);
        let size = video * scale;
        if self.scale.is_none() {
            return Rect::from_center_size(view.center(), size);
        }
        let wanted_min = view.center() - self.centre.to_vec2() * scale;
        let hold = |wanted: f32, size: f32, start: f32, length: f32| {
            if size <= length {
                // Smaller than the view this way, so it stays in the middle.
                start + (length - size) / 2.0
            } else {
                wanted.clamp(start + length - size, start)
            }
        };
        let left = hold(wanted_min.x, size.x, view.left(), view.width());
        let top = hold(wanted_min.y, size.y, view.top(), view.height());
        Rect::from_min_size(pos2(left, top), size)
    }

    /// The part of the video that shows, in its own pixels.
    #[cfg(test)]
    pub fn visible_part(&self, view: Rect, video: Vec2) -> Rect {
        let picture = self.picture_rect(view, video);
        let scale = self.scale(view, video);
        let shown = picture.intersect(view);
        Rect::from_min_max(
            ((shown.min - picture.min) / scale).to_pos2(),
            ((shown.max - picture.min) / scale).to_pos2(),
        )
    }

    /// Sets a scale and works out the middle so the picture is where `picture` says, then
    /// settles anything out of range.
    fn settle(view: Rect, video: Vec2, scale: f32, picture_min: Pos2, ppp: f32) -> Self {
        let fit = fit_scale(view.size(), video);
        let scale = scale.clamp(fit * LEAST_OF_FIT, Self::most(view, video, ppp));
        if (scale - fit).abs() <= fit * 0.0001 {
            return Self::fit();
        }
        if scale < fit {
            // Smaller than fit, it sits in the middle and can't be panned.
            return Self {
                out: Some(scale / fit),
                ..Self::fit()
            };
        }
        let centre = ((view.center() - picture_min) / scale).to_pos2();
        let zoom = Self {
            scale: Some(scale),
            out: None,
            centre,
        };
        // Holding the picture in the view moves the middle, which is kept so panning back
        // starts from where it shows.
        let held = zoom.picture_rect(view, video);
        Self {
            scale: Some(scale),
            out: None,
            centre: ((view.center() - held.min) / scale).to_pos2(),
        }
    }

    /// Zooms by a factor, keeping the point of the video under `toward` where it is. Passing
    /// through fit stops there, so it is easy to land on.
    pub fn zoom_by(&self, view: Rect, video: Vec2, factor: f32, toward: Pos2, ppp: f32) -> Self {
        let picture = self.picture_rect(view, video);
        let scale = self.scale(view, video);
        let fit = fit_scale(view.size(), video);
        if (scale - fit) * (scale * factor - fit) < 0.0 {
            return Self::fit();
        }
        let under = (toward - picture.min) / scale;
        let new_scale = scale * factor;
        let new_min = toward - under * new_scale;
        Self::settle(view, video, new_scale, new_min, ppp)
    }

    /// Zooms to a percentage of real pixels, around the middle of the view.
    pub fn zoom_to_percent(&self, view: Rect, video: Vec2, percent: f32, ppp: f32) -> Self {
        let scale = self.scale(view, video);
        let wanted = percent / 100.0 / ppp;
        self.zoom_by(view, video, wanted / scale, view.center(), ppp)
    }

    /// Moves the picture by a drag, held inside the view.
    pub fn pan(&self, view: Rect, video: Vec2, by: Vec2, ppp: f32) -> Self {
        if self.scale.is_none() {
            return *self;
        }
        let picture = self.picture_rect(view, video);
        Self::settle(view, video, self.scale(view, video), picture.min + by, ppp)
    }

    /// The zoom after the view changes size: zoomed in, the same scale, still held in, or fit
    /// if that is now larger; zoomed out, the same share of the new fit.
    pub fn for_view(&self, view: Rect, video: Vec2, ppp: f32) -> Self {
        let Some(scale) = self.scale else {
            return *self;
        };
        // Zoomed in, a window grown past it goes back to fit rather than to zoomed out.
        if scale <= fit_scale(view.size(), video) {
            return Self::fit();
        }
        let picture_min = view.center() - self.centre.to_vec2() * scale;
        Self::settle(view, video, scale, picture_min, ppp)
    }

    /// Whether the picture on screen is larger than `preview` pixels across, so the video
    /// should be decoded at full size to stay sharp.
    pub fn wants_sharp(&self, view: Rect, video: Vec2, preview: Vec2, ppp: f32) -> bool {
        if self.is_fit() {
            return false;
        }
        let shown = video * self.scale(view, video) * ppp;
        shown.x > preview.x + 0.5 || shown.y > preview.y + 0.5
    }
}

/// Where a zoom sits along the zoom slider, from 0 to 1: fit in the middle, a quarter of fit at
/// the left end and the closest zoom at the right, each half evenly in steps of zoom.
pub fn slider_share(percent: f32, fit: f32, most: f32) -> f32 {
    if percent <= fit {
        let least = fit * LEAST_OF_FIT;
        0.5 * (percent.max(least) / least).ln() / (fit / least).ln()
    } else if most > fit {
        0.5 + 0.5 * (percent.min(most) / fit).ln() / (most / fit).ln()
    } else {
        0.5
    }
}

/// The zoom at a point along the slider, the other way from [`slider_share`].
pub fn percent_at_share(share: f32, fit: f32, most: f32) -> f32 {
    let share = share.clamp(0.0, 1.0);
    if share <= 0.5 {
        let least = fit * LEAST_OF_FIT;
        least * (fit / least).powf(share / 0.5)
    } else {
        fit * (most.max(fit) / fit).powf((share - 0.5) / 0.5)
    }
}

/// The scale of a fitted picture, which the percentage shows before anything is zoomed.
pub fn fit_percent(view: Rect, video: Vec2, ppp: f32) -> f32 {
    fit_scale(view.size(), video) * ppp * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;

    fn view() -> Rect {
        Rect::from_min_size(pos2(0.0, 50.0), vec2(1000.0, 500.0))
    }

    const VIDEO: Vec2 = vec2(2000.0, 1000.0);

    fn close(a: Pos2, b: Pos2) -> bool {
        (a - b).length() < 0.01
    }

    #[test]
    fn fit_shows_the_whole_picture_as_large_as_the_view_allows() {
        let zoom = Zoom::fit();
        assert_eq!(zoom.scale(view(), VIDEO), 0.5);
        assert_eq!(zoom.picture_rect(view(), VIDEO), view());
        assert_eq!(zoom.percent(view(), VIDEO, 1.0), 50.0);
        assert_eq!(zoom.percent(view(), VIDEO, 1.25), 62.5, "real pixels count");

        let tall = vec2(500.0, 1000.0);
        let rect = zoom.picture_rect(view(), tall);
        assert_eq!(rect.size(), vec2(250.0, 500.0));
        assert_eq!(rect.center(), view().center());
    }

    #[test]
    fn zooming_toward_a_point_keeps_that_point_under_the_pointer() {
        for toward in [view().min, view().max, pos2(700.0, 200.0), view().center()] {
            let before = Zoom::fit();
            let picture = before.picture_rect(view(), VIDEO);
            let under = (toward - picture.min) / before.scale(view(), VIDEO);

            let after = before.zoom_by(view(), VIDEO, 3.0, toward, 1.0);
            let picture = after.picture_rect(view(), VIDEO);
            let now = picture.min + under * after.scale(view(), VIDEO);

            assert!(close(now, toward), "{toward:?} moved to {now:?}");
            assert_eq!(after.scale(view(), VIDEO), 1.5);
        }
    }

    #[test]
    fn zoom_stays_between_a_quarter_of_fit_and_eight_times_real_pixels() {
        let zoomed_out = Zoom::fit().zoom_by(view(), VIDEO, 0.1, view().center(), 1.0);
        assert_eq!(
            zoomed_out.percent(view(), VIDEO, 1.0),
            12.5,
            "never smaller than a quarter of fit"
        );
        let rect = zoomed_out.picture_rect(view(), VIDEO);
        assert_eq!(rect.size(), view().size() * 0.25);
        assert_eq!(
            rect.center(),
            view().center(),
            "smaller, it sits in the middle"
        );
        assert!(!zoomed_out.is_zoomed_in(view(), VIDEO));
        assert!(!zoomed_out.is_fit());

        let mut zoom = Zoom::fit();
        for _ in 0..40 {
            zoom = zoom.zoom_by(view(), VIDEO, STEP, view().center(), 1.25);
        }
        assert!((zoom.percent(view(), VIDEO, 1.25) - 800.0).abs() < 0.01);

        let back = zoom.zoom_to_percent(view(), VIDEO, 10.0, 1.25);
        assert!(back.is_fit());
        let exact = Zoom::fit().zoom_to_percent(view(), VIDEO, 200.0, 1.0);
        assert!((exact.percent(view(), VIDEO, 1.0) - 200.0).abs() < 0.01);
    }

    #[test]
    fn zoomed_out_the_picture_keeps_its_share_of_fit_as_the_window_changes() {
        let out = Zoom::fit().zoom_by(view(), VIDEO, 0.5, view().center(), 1.0);
        assert_eq!(out.scale(view(), VIDEO), 0.25);
        let wider = Rect::from_min_size(pos2(0.0, 50.0), vec2(2000.0, 1000.0));
        let kept = out.for_view(wider, VIDEO, 1.0);
        assert_eq!(kept.scale(wider, VIDEO), 0.5, "half of the new fit");
        assert_eq!(kept.picture_rect(wider, VIDEO).center(), wider.center());
        assert_eq!(
            out.pan(view(), VIDEO, vec2(80.0, 0.0), 1.0),
            out,
            "it can't be panned"
        );
    }

    #[test]
    fn passing_through_fit_stops_there() {
        let zoomed_in = Zoom::fit().zoom_by(view(), VIDEO, 1.1, view().center(), 1.0);
        assert!(zoomed_in.is_zoomed_in(view(), VIDEO));
        assert!(
            zoomed_in
                .zoom_by(view(), VIDEO, 0.5, view().center(), 1.0)
                .is_fit(),
            "zooming out from just past fit stops at fit"
        );
        let zoomed_out = Zoom::fit().zoom_by(view(), VIDEO, 0.9, view().center(), 1.0);
        assert!(
            zoomed_out
                .zoom_by(view(), VIDEO, 2.0, view().center(), 1.0)
                .is_fit(),
            "and so does zooming in from just under it"
        );
        let further = Zoom::fit().zoom_by(view(), VIDEO, 0.8, view().center(), 1.0);
        assert_eq!(further.scale(view(), VIDEO), 0.4, "from fit it carries on");
    }

    #[test]
    fn the_slider_has_fit_in_the_middle_and_goes_both_ways_evenly() {
        let (fit, most) = (50.0, 800.0);
        assert_eq!(slider_share(fit, fit, most), 0.5);
        assert!(
            (slider_share(12.5, fit, most)).abs() < 1e-6,
            "a quarter of fit"
        );
        assert!((slider_share(800.0, fit, most) - 1.0).abs() < 1e-6);
        assert!(
            (slider_share(25.0, fit, most) - 0.25).abs() < 1e-6,
            "half of fit"
        );
        for share in [0.0, 0.1, 0.25, 0.5, 0.7, 1.0] {
            let percent = percent_at_share(share, fit, most);
            assert!(
                (slider_share(percent, fit, most) - share).abs() < 1e-5,
                "{share} goes to {percent} and back"
            );
        }
        assert_eq!(percent_at_share(-1.0, fit, most), 12.5);
        assert!((percent_at_share(2.0, fit, most) - most).abs() < 1e-3);
    }

    #[test]
    fn a_video_smaller_than_the_view_fits_larger_than_its_pixels_and_cant_zoom_in() {
        let small = vec2(100.0, 50.0);
        let zoom = Zoom::fit();
        assert_eq!(zoom.scale(view(), small), 10.0);
        let in_more = zoom.zoom_by(view(), small, 2.0, view().center(), 1.0);
        assert!(
            in_more.is_fit(),
            "fit is already past 8x, so there's nowhere to go"
        );
    }

    #[test]
    fn panning_moves_the_picture_but_never_out_of_the_view() {
        let zoom = Zoom::fit().zoom_by(view(), VIDEO, 2.0, view().center(), 1.0);
        let before = zoom.picture_rect(view(), VIDEO);

        let moved = zoom.pan(view(), VIDEO, vec2(-100.0, 30.0), 1.0);
        let after = moved.picture_rect(view(), VIDEO);
        assert!(close(after.min, before.min + vec2(-100.0, 30.0)));

        let far = zoom.pan(view(), VIDEO, vec2(5000.0, 5000.0), 1.0);
        let rect = far.picture_rect(view(), VIDEO);
        assert_eq!(rect.min, view().min, "held at its top left");
        let far = zoom.pan(view(), VIDEO, vec2(-5000.0, -5000.0), 1.0);
        let rect = far.picture_rect(view(), VIDEO);
        assert_eq!(rect.max, view().max, "held at its bottom right");
        assert!(
            view().intersect(rect) == view(),
            "it always covers the view"
        );

        assert_eq!(
            Zoom::fit().pan(view(), VIDEO, vec2(50.0, 0.0), 1.0),
            Zoom::fit()
        );
    }

    #[test]
    fn the_visible_part_is_what_the_view_covers_in_the_videos_pixels() {
        assert_eq!(
            Zoom::fit().visible_part(view(), VIDEO),
            Rect::from_min_size(pos2(0.0, 0.0), VIDEO)
        );
        let zoom = Zoom::fit().zoom_by(view(), VIDEO, 2.0, view().min, 1.0);
        let part = zoom.visible_part(view(), VIDEO);
        assert!(close(part.min, pos2(0.0, 0.0)));
        assert!(close(part.max, pos2(1000.0, 500.0)));
    }

    #[test]
    fn a_resized_view_keeps_the_zoom_held_in_or_goes_back_to_fit() {
        let zoom = Zoom::fit().zoom_by(view(), VIDEO, 2.0, view().max, 1.0);
        let wider = Rect::from_min_size(pos2(0.0, 50.0), vec2(1600.0, 800.0));
        let kept = zoom.for_view(wider, VIDEO, 1.0);
        assert_eq!(kept.scale(wider, VIDEO), 1.0);
        let rect = kept.picture_rect(wider, VIDEO);
        assert!(wider.intersect(rect) == wider);

        let huge = Rect::from_min_size(pos2(0.0, 0.0), vec2(4000.0, 2000.0));
        assert!(
            zoom.for_view(huge, VIDEO, 1.0).is_fit(),
            "fit is now larger"
        );
        assert!(Zoom::fit().for_view(huge, VIDEO, 1.0).is_fit());
    }

    #[test]
    fn the_video_is_decoded_sharp_only_once_shown_larger_than_the_preview() {
        let preview = vec2(1280.0, 640.0);
        assert!(!Zoom::fit().wants_sharp(view(), VIDEO, preview, 1.0));
        let a_little = Zoom::fit().zoom_to_percent(view(), VIDEO, 60.0, 1.0);
        assert!(
            !a_little.wants_sharp(view(), VIDEO, preview, 1.0),
            "1200 wide"
        );
        let more = Zoom::fit().zoom_to_percent(view(), VIDEO, 70.0, 1.0);
        assert!(more.wants_sharp(view(), VIDEO, preview, 1.0), "1400 wide");
    }
}
