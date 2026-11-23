//! CoreGraphics 几何类型。CGEvent 的位置、窗口边界、AX 的 frame 都用它们，
//! 坐标系一致：原点在主显示器左上角，单位是点（不是像素）。

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

impl CGRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }

    /// 半开区间：右边界和下边界不算在内，相邻图标不会同时命中。
    pub fn contains(&self, p: CGPoint) -> bool {
        p.x >= self.origin.x
            && p.x < self.origin.x + self.size.width
            && p.y >= self.origin.y
            && p.y < self.origin.y + self.size.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_is_half_open() {
        let r = CGRect::new(166.0, 1000.0, 59.0, 75.0);
        assert!(r.contains(CGPoint {
            x: 166.0,
            y: 1000.0
        }));
        assert!(r.contains(CGPoint {
            x: 195.5,
            y: 1040.0
        }));
        assert!(
            !r.contains(CGPoint {
                x: 225.0,
                y: 1040.0
            }),
            "右边界归下一个图标"
        );
        assert!(
            !r.contains(CGPoint {
                x: 195.5,
                y: 1075.0
            }),
            "下边界不算"
        );
        assert!(!r.contains(CGPoint {
            x: 165.9,
            y: 1040.0
        }));
    }

    #[test]
    fn layout_matches_c() {
        assert_eq!(std::mem::size_of::<CGPoint>(), 16);
        assert_eq!(std::mem::size_of::<CGRect>(), 32);
    }
}
