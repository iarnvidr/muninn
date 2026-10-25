//! Pure geometry for placing the native YouTube web view over the chrome's content pane.
//!
//! The chrome UI reports its pane in CSS pixels together with its own CSS viewport size.
//! The native child web view is positioned in the window's logical points. One uniform
//! scale (chrome logical width / CSS viewport width) and the chrome web view's own origin
//! map between the two. The window's inner size is deliberately NOT used: on macOS it can
//! include the title bar (28 pt), which stretched the vertical axis and pushed the child
//! over the toolbar.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bounds {
    fn finite(&self) -> bool {
        [self.x, self.y, self.w, self.h]
            .iter()
            .all(|n| n.is_finite())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapped {
    pub rect: Bounds,
    /// The single scale applied to both axes (native points per CSS pixel).
    pub scale: f64,
    /// True when the rectangle had to be cut back to the CSS viewport.
    pub clamped: bool,
}

pub const MIN_SCALE: f64 = 0.25;
pub const MAX_SCALE: f64 = 4.0;

fn valid_extent(n: Option<f64>) -> Option<f64> {
    n.filter(|n| n.is_finite() && *n > 0.0)
}

/// Maps the pane `css` (CSS pixels, relative to the chrome viewport) to native logical points.
///
/// `chrome` is the chrome web view's own logical bounds (origin and size) when known.
/// `viewport_w`/`viewport_h` are the chrome's CSS viewport (`innerWidth`/`innerHeight`).
/// Returns `None` for unusable input (non-finite or negative values).
pub fn map_viewport_rect(
    css: Bounds,
    viewport_w: Option<f64>,
    viewport_h: Option<f64>,
    chrome: Option<Bounds>,
) -> Option<Mapped> {
    if !css.finite() || css.w < 0.0 || css.h < 0.0 {
        return None;
    }
    let chrome = chrome.filter(|c| c.finite() && c.w > 0.0 && c.h > 0.0);
    let (vw, vh) = (valid_extent(viewport_w), valid_extent(viewport_h));
    // One scale for both axes, taken from the width: the height can carry title-bar or
    // resize-transient differences that must not stretch the page.
    let scale = match (chrome, vw, vh) {
        (Some(c), Some(vw), _) => c.w / vw,
        (Some(c), None, Some(vh)) => c.h / vh,
        _ => 1.0,
    }
    .clamp(MIN_SCALE, MAX_SCALE);
    let (ox, oy) = chrome.map(|c| (c.x, c.y)).unwrap_or((0.0, 0.0));
    let mut left = css.x;
    let mut top = css.y;
    let mut right = css.x + css.w;
    let mut bottom = css.y + css.h;
    let mut clamped = false;
    // Keep the pane inside the CSS viewport it was measured in.
    let mut cut = |v: &mut f64, lo: f64, hi: Option<f64>| {
        let before = *v;
        *v = v.max(lo);
        if let Some(hi) = hi {
            *v = v.min(hi);
        }
        if *v != before {
            clamped = true;
        }
    };
    cut(&mut left, 0.0, vw);
    cut(&mut right, 0.0, vw);
    cut(&mut top, 0.0, vh);
    cut(&mut bottom, 0.0, vh);
    let rect = Bounds {
        x: ox + left * scale,
        y: oy + top * scale,
        w: (right - left).max(0.0) * scale,
        h: (bottom - top).max(0.0) * scale,
    };
    Some(Mapped {
        rect,
        scale,
        clamped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }
    fn chrome(w: f64, h: f64) -> Option<Bounds> {
        Some(Bounds {
            x: 0.0,
            y: 0.0,
            w,
            h,
        })
    }

    #[test]
    fn observed_1360_viewport_has_no_28px_overflow() {
        // Pane below 34px tabs + 38px toolbar in a 1360x832 CSS viewport, sidebar 224px.
        let css = Bounds {
            x: 224.0,
            y: 72.0,
            w: 1136.0,
            h: 760.0,
        };
        let m = map_viewport_rect(css, Some(1360.0), Some(832.0), chrome(1360.0, 832.0)).unwrap();
        assert!(close(m.rect.y, 72.0) && close(m.rect.h, 760.0));
        assert!(close(m.rect.y + m.rect.h, 832.0));
        assert!(close(m.rect.x + m.rect.w, 1360.0));
        assert!(!m.clamped && close(m.scale, 1.0));
    }

    #[test]
    fn title_bar_height_in_window_inner_size_is_ignored() {
        // The old code divided a 860pt window height by the 832px viewport (scale_y 1.0337).
        // The chrome web view reports 1360 wide, so the scale is 1 whatever its height says.
        let css = Bounds {
            x: 224.0,
            y: 72.4,
            w: 1136.0,
            h: 759.6,
        };
        let m = map_viewport_rect(css, Some(1360.0), Some(832.0), chrome(1360.0, 860.0)).unwrap();
        assert!(close(m.rect.y, 72.4));
        assert!(
            close(m.rect.y + m.rect.h, 832.0),
            "bottom {}",
            m.rect.y + m.rect.h
        );
        assert!(close(m.scale, 1.0));
    }

    #[test]
    fn observed_1512_viewport() {
        let css = Bounds {
            x: 52.0,
            y: 72.0,
            w: 1460.0,
            h: 848.0,
        };
        let m = map_viewport_rect(css, Some(1512.0), Some(920.0), chrome(1512.0, 948.0)).unwrap();
        assert!(close(m.rect.y, 72.0) && close(m.rect.y + m.rect.h, 920.0));
        assert!(close(m.rect.x + m.rect.w, 1512.0));
    }

    #[test]
    fn resize_transient_is_uniform_and_stays_inside_the_pane() {
        // Chrome already shrank to 1224 wide while the page still reports 1360 CSS px.
        let css = Bounds {
            x: 224.0,
            y: 72.0,
            w: 1136.0,
            h: 760.0,
        };
        let m = map_viewport_rect(css, Some(1360.0), Some(832.0), chrome(1224.0, 780.0)).unwrap();
        assert!(close(m.scale, 1224.0 / 1360.0));
        assert!(
            close(m.rect.w / css.w, m.rect.h / css.h),
            "axes must not be stretched independently"
        );
        assert!(close(m.rect.y, 72.0 * m.scale));
        assert!(m.rect.x + m.rect.w <= 1224.0 + 0.01);
    }

    #[test]
    fn zoom_scales_uniformly() {
        // 125% page zoom: CSS viewport is 1088x665.6 for a 1360x832 point chrome.
        let css = Bounds {
            x: 179.2,
            y: 57.6,
            w: 908.8,
            h: 608.0,
        };
        let m = map_viewport_rect(css, Some(1088.0), Some(665.6), chrome(1360.0, 832.0)).unwrap();
        assert!(close(m.scale, 1.25));
        assert!(close(m.rect.x, 224.0) && close(m.rect.y, 72.0));
        assert!(close(m.rect.x + m.rect.w, 1360.0) && close(m.rect.y + m.rect.h, 832.0));
    }

    #[test]
    fn display_scale_does_not_matter_in_logical_points() {
        // Retina: physical sizes are converted to logical points before the mapping,
        // so the same logical chrome yields the same rectangle at any backing scale.
        let css = Bounds {
            x: 52.0,
            y: 72.0,
            w: 1460.0,
            h: 848.0,
        };
        let a = map_viewport_rect(css, Some(1512.0), Some(920.0), chrome(1512.0, 920.0)).unwrap();
        let b = map_viewport_rect(
            css,
            Some(1512.0),
            Some(920.0),
            chrome(3024.0 / 2.0, 1840.0 / 2.0),
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn chrome_origin_is_the_content_origin() {
        let css = Bounds {
            x: 52.0,
            y: 72.0,
            w: 100.0,
            h: 100.0,
        };
        let c = Some(Bounds {
            x: 0.0,
            y: 28.0,
            w: 1512.0,
            h: 920.0,
        });
        let m = map_viewport_rect(css, Some(1512.0), Some(920.0), c).unwrap();
        assert!(close(m.rect.y, 100.0) && close(m.rect.x, 52.0));
    }

    #[test]
    fn nonzero_origin_offsets_the_rect_and_not_its_size() {
        let css = Bounds {
            x: 224.0,
            y: 72.0,
            w: 1136.0,
            h: 760.0,
        };
        let c = Some(Bounds {
            x: 10.0,
            y: 28.0,
            w: 1360.0,
            h: 832.0,
        });
        let m = map_viewport_rect(css, Some(1360.0), Some(832.0), c).unwrap();
        assert!(close(m.rect.x, 234.0) && close(m.rect.y, 100.0));
        assert!(close(m.rect.w, 1136.0) && close(m.rect.h, 760.0));
        assert!(close(m.rect.x + m.rect.w, 10.0 + 1360.0));
        assert!(close(m.rect.y + m.rect.h, 28.0 + 832.0));
    }

    #[test]
    fn nonzero_origin_is_added_after_scaling_and_clamping() {
        // 125% zoom with a chrome at (6, 28): the origin is in points and is not scaled.
        let css = Bounds {
            x: 179.2,
            y: 57.6,
            w: 908.8,
            h: 700.0,
        };
        let c = Some(Bounds {
            x: 6.0,
            y: 28.0,
            w: 1360.0,
            h: 832.0,
        });
        let m = map_viewport_rect(css, Some(1088.0), Some(665.6), c).unwrap();
        assert!(m.clamped && close(m.scale, 1.25));
        assert!(close(m.rect.x, 230.0) && close(m.rect.y, 100.0));
        assert!(close(m.rect.y + m.rect.h, 28.0 + 832.0));
    }

    #[test]
    fn negative_origin_is_kept() {
        let css = Bounds {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let c = Some(Bounds {
            x: -5.0,
            y: -3.0,
            w: 500.0,
            h: 500.0,
        });
        let m = map_viewport_rect(css, Some(500.0), Some(500.0), c).unwrap();
        assert!(close(m.rect.x, -5.0) && close(m.rect.y, -3.0));
    }

    #[test]
    fn overflow_is_clamped_to_the_viewport_below_the_toolbar() {
        let css = Bounds {
            x: 224.0,
            y: 72.0,
            w: 1136.0,
            h: 900.0,
        };
        let m = map_viewport_rect(css, Some(1360.0), Some(832.0), chrome(1360.0, 832.0)).unwrap();
        assert!(m.clamped);
        assert!(close(m.rect.y + m.rect.h, 832.0));
        let above = Bounds {
            x: 224.0,
            y: -10.0,
            w: 100.0,
            h: 100.0,
        };
        let m = map_viewport_rect(above, Some(1360.0), Some(832.0), chrome(1360.0, 832.0)).unwrap();
        assert!(m.rect.y >= 0.0 && m.clamped);
    }

    #[test]
    fn carousel_offset_pane_stays_below_the_carousel() {
        // Favourites carousel (200px) pushes the pane to y=272.
        let css = Bounds {
            x: 224.0,
            y: 272.0,
            w: 1136.0,
            h: 560.0,
        };
        let m = map_viewport_rect(css, Some(1360.0), Some(832.0), chrome(1360.0, 832.0)).unwrap();
        assert!(close(m.rect.y, 272.0) && close(m.rect.y + m.rect.h, 832.0));
    }

    #[test]
    fn missing_data_falls_back_to_identity_and_bad_data_is_rejected() {
        let css = Bounds {
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
        };
        let m = map_viewport_rect(css, None, None, None).unwrap();
        assert_eq!(m.rect, css);
        assert!(map_viewport_rect(Bounds { w: f64::NAN, ..css }, None, None, None).is_none());
        assert!(map_viewport_rect(Bounds { h: -1.0, ..css }, None, None, None).is_none());
        let m = map_viewport_rect(css, Some(0.0), Some(f64::NAN), chrome(100.0, 100.0)).unwrap();
        assert_eq!(m.scale, 1.0);
    }
}
