use imgui::Ui;

pub const MAX_SCALE: f32 = 8.0;
const WHEEL_STEP: f32 = 1.25;
const DOUBLE_CLICK_SCALE: f32 = 2.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    pub scale: f32,
    pub center: [f32; 2],
}

impl Default for Zoom {
    fn default() -> Self {
        Self { scale: 1.0, center: [0.5, 0.5] }
    }
}

impl Zoom {
    pub fn zoomed(&self) -> bool {
        self.scale > 1.001
    }

    pub fn uv(&self) -> ([f32; 2], [f32; 2]) {
        let half = 0.5 / self.scale;
        ([self.center[0] - half, self.center[1] - half], [self.center[0] + half, self.center[1] + half])
    }

    pub fn zoom_at(&mut self, factor: f32, at: [f32; 2]) {
        let size = 1.0 / self.scale;
        let point = [self.center[0] - size * 0.5 + at[0] * size, self.center[1] - size * 0.5 + at[1] * size];
        self.scale = (self.scale * factor).clamp(1.0, MAX_SCALE);
        let size = 1.0 / self.scale;
        self.center = [point[0] - at[0] * size + size * 0.5, point[1] - at[1] * size + size * 0.5];
        self.clamp();
    }

    pub fn pan(&mut self, delta: [f32; 2]) {
        let size = 1.0 / self.scale;
        self.center = [self.center[0] - delta[0] * size, self.center[1] - delta[1] * size];
        self.clamp();
    }

    fn clamp(&mut self) {
        let half = 0.5 / self.scale;
        self.center = [self.center[0].clamp(half, 1.0 - half), self.center[1].clamp(half, 1.0 - half)];
    }

    pub fn interact(&mut self, ui: &Ui, min: [f32; 2], max: [f32; 2]) -> bool {
        let size = [(max[0] - min[0]).max(1.0), (max[1] - min[1]).max(1.0)];
        let hovered = ui.is_window_hovered() && ui.is_mouse_hovering_rect(min, max);
        if !hovered {
            return false;
        }
        let mouse = ui.io().mouse_pos;
        let at = [((mouse[0] - min[0]) / size[0]).clamp(0.0, 1.0), ((mouse[1] - min[1]) / size[1]).clamp(0.0, 1.0)];
        let wheel = ui.io().mouse_wheel;
        if wheel != 0.0 {
            self.zoom_at(WHEEL_STEP.powf(wheel), at);
        }
        if ui.is_mouse_double_clicked(imgui::MouseButton::Left) {
            if self.zoomed() {
                *self = Self::default();
            } else {
                self.zoom_at(DOUBLE_CLICK_SCALE, at);
            }
        } else if self.zoomed() && ui.is_mouse_down(imgui::MouseButton::Left) {
            let delta = ui.io().mouse_delta;
            self.pan([delta[0] / size[0], delta[1] / size[1]]);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let mut zoom = Zoom::default();
        zoom.zoom_at(2.0, [0.75, 0.25]);
        let (min, max) = zoom.uv();
        let under = [min[0] + 0.75 * (max[0] - min[0]), min[1] + 0.25 * (max[1] - min[1])];
        assert!(close(under, [0.75, 0.25]));
        assert!((max[0] - min[0] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn the_view_never_leaves_the_picture() {
        let mut zoom = Zoom::default();
        zoom.zoom_at(4.0, [1.0, 1.0]);
        zoom.pan([-10.0, -10.0]);
        let (min, max) = zoom.uv();
        assert!(close(max, [1.0, 1.0]));
        assert!(min[0] >= 0.0 && min[1] >= 0.0);
        zoom.zoom_at(0.01, [0.5, 0.5]);
        assert_eq!(zoom, Zoom::default());
    }

    #[test]
    fn scale_stops_at_the_limit() {
        let mut zoom = Zoom::default();
        for _ in 0..40 {
            zoom.zoom_at(WHEEL_STEP, [0.5, 0.5]);
        }
        assert_eq!(zoom.scale, MAX_SCALE);
    }
}
