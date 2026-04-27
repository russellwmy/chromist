/// An (x, y) position in CSS pixels.
///
/// # Examples
///
/// ```
/// use chromist::Point;
/// let p = Point::new(10.0, 20.0) + Point::new(1.0, 2.0);
/// assert_eq!(p, Point::new(11.0, 22.0));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// X coordinate in CSS pixels.
    pub x: f64,
    /// Y coordinate in CSS pixels.
    pub y: f64,
}

impl Point {
    /// Construct a point at `(x, y)`.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Axis-aligned rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width in CSS pixels.
    pub width: f64,
    /// Height in CSS pixels.
    pub height: f64,
}

impl BoundingBox {
    /// Construct from origin and dimensions.
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    /// Returns the centre point of the rectangle.
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Decode the 8-number quad `[x1, y1, x2, y2, x3, y3, x4, y4]` returned by
    /// CDP into an axis-aligned bounding box.
    pub fn from_quad(quad: &[f64]) -> Option<Self> {
        if quad.len() < 8 {
            return None;
        }
        let xs = [quad[0], quad[2], quad[4], quad[6]];
        let ys = [quad[1], quad[3], quad[5], quad[7]];
        let min_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_y = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Some(BoundingBox { x: min_x, y: min_y, width: max_x - min_x, height: max_y - min_y })
    }
}

/// Device viewport configuration applied via `Emulation.setDeviceMetricsOverride`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: u32,
    /// Viewport height in CSS pixels.
    pub height: u32,
    /// Device pixel ratio (Retina = 2.0, 4K = 1.0–2.0, etc.).
    pub device_scale_factor: f64,
    /// Pretend to be a mobile device (UA touch handling, viewport meta).
    pub emulating_mobile: bool,
    /// Force landscape orientation.
    pub is_landscape: bool,
    /// Enable touch event emulation.
    pub has_touch: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            width: 1280,
            height: 720,
            device_scale_factor: 1.0,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        }
    }
}

impl Viewport {
    /// Construct with the given width × height; other fields take defaults.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, ..Default::default() }
    }
}

impl std::ops::Add for Point {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Point::new(self.x + rhs.x, self.y + rhs.y)
    }
}
impl std::ops::Sub for Point {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Point::new(self.x - rhs.x, self.y - rhs.y)
    }
}
impl std::ops::Div<f64> for Point {
    type Output = Self;
    fn div(self, rhs: f64) -> Self {
        Point::new(self.x / rhs, self.y / rhs)
    }
}

/// Four-corner polygon in CSS pixels — CDP's native shape for element
/// quads (potentially rotated rectangles).
#[derive(Debug, Clone, PartialEq)]
pub struct ElementQuad {
    /// Top-left vertex.
    pub top_left: Point,
    /// Top-right vertex.
    pub top_right: Point,
    /// Bottom-right vertex.
    pub bottom_right: Point,
    /// Bottom-left vertex.
    pub bottom_left: Point,
}

impl ElementQuad {
    /// Parse an 8-number flat array `[x1,y1,x2,y2,x3,y3,x4,y4]` from CDP.
    /// Returns `None` if fewer than 8 elements are provided.
    pub fn from_raw(quad: &[f64]) -> Option<Self> {
        if quad.len() < 8 {
            return None;
        }
        Some(Self {
            top_left: Point::new(quad[0], quad[1]),
            top_right: Point::new(quad[2], quad[3]),
            bottom_right: Point::new(quad[4], quad[5]),
            bottom_left: Point::new(quad[6], quad[7]),
        })
    }

    /// Returns the centroid (mean of the four vertices).
    pub fn quad_center(&self) -> Point {
        Point::new(
            (self.top_left.x + self.top_right.x + self.bottom_right.x + self.bottom_left.x) / 4.0,
            (self.top_left.y + self.top_right.y + self.bottom_right.y + self.bottom_left.y) / 4.0,
        )
    }

    /// Returns the area of the quad via the diagonal-product formula.
    pub fn quad_area(&self) -> f64 {
        let d1 = ((self.bottom_right.x - self.top_left.x).powi(2)
            + (self.bottom_right.y - self.top_left.y).powi(2))
        .sqrt();
        let d2 = ((self.bottom_left.x - self.top_right.x).powi(2)
            + (self.bottom_left.y - self.top_right.y).powi(2))
        .sqrt();
        d1 * d2 / 2.0
    }

    fn xs(&self) -> [f64; 4] {
        [self.top_left.x, self.top_right.x, self.bottom_right.x, self.bottom_left.x]
    }

    fn ys(&self) -> [f64; 4] {
        [self.top_left.y, self.top_right.y, self.bottom_right.y, self.bottom_left.y]
    }

    /// Returns `max_x − min_x` across the four vertices.
    pub fn width(&self) -> f64 {
        let xs = self.xs();
        xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - xs.iter().cloned().fold(f64::INFINITY, f64::min)
    }

    /// Returns `max_y − min_y` across the four vertices.
    pub fn height(&self) -> f64 {
        let ys = self.ys();
        ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - ys.iter().cloned().fold(f64::INFINITY, f64::min)
    }

    /// Returns `width() / height()`.
    pub fn aspect_ratio(&self) -> f64 {
        self.width() / self.height()
    }

    /// Returns the smallest `x` across the four vertices.
    pub fn most_left(&self) -> f64 {
        self.xs().iter().cloned().fold(f64::INFINITY, f64::min)
    }

    /// Returns the largest `x` across the four vertices.
    pub fn most_right(&self) -> f64 {
        self.xs().iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    }

    /// Returns the smallest `y` across the four vertices.
    pub fn most_top(&self) -> f64 {
        self.ys().iter().cloned().fold(f64::INFINITY, f64::min)
    }

    /// Returns the largest `y` across the four vertices.
    pub fn most_bottom(&self) -> f64 {
        self.ys().iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    }

    /// `true` when this quad ends above `other` begins (no overlap).
    pub fn strictly_above(&self, other: &ElementQuad) -> bool {
        self.most_bottom() < other.most_top()
    }

    /// `true` when this quad does not extend below `other`'s top edge
    /// (touching counts).
    pub fn above(&self, other: &ElementQuad) -> bool {
        self.most_bottom() <= other.most_top()
    }

    /// `true` when this quad starts below `other` ends (no overlap).
    pub fn strictly_below(&self, other: &ElementQuad) -> bool {
        self.most_top() > other.most_bottom()
    }

    /// `true` when this quad does not extend above `other`'s bottom edge.
    pub fn below(&self, other: &ElementQuad) -> bool {
        self.most_top() >= other.most_bottom()
    }

    /// `true` when this quad ends to the left of `other` (no overlap).
    pub fn strictly_left_of(&self, other: &ElementQuad) -> bool {
        self.most_right() < other.most_left()
    }

    /// `true` when this quad does not extend past `other`'s left edge.
    pub fn left_of(&self, other: &ElementQuad) -> bool {
        self.most_right() <= other.most_left()
    }

    /// `true` when this quad starts to the right of `other` (no overlap).
    pub fn strictly_right_of(&self, other: &ElementQuad) -> bool {
        self.most_left() > other.most_right()
    }

    /// `true` when this quad does not extend before `other`'s right edge.
    pub fn right_of(&self, other: &ElementQuad) -> bool {
        self.most_left() >= other.most_right()
    }

    /// `true` when this quad fits entirely within `other`'s x-range.
    pub fn within_horizontal_bounds_of(&self, other: &ElementQuad) -> bool {
        self.most_left() >= other.most_left() && self.most_right() <= other.most_right()
    }

    /// `true` when this quad fits entirely within `other`'s y-range.
    pub fn within_vertical_bounds_of(&self, other: &ElementQuad) -> bool {
        self.most_top() >= other.most_top() && self.most_bottom() <= other.most_bottom()
    }

    /// `true` when this quad is fully contained inside `other`.
    pub fn within_bounds_of(&self, other: &ElementQuad) -> bool {
        self.within_horizontal_bounds_of(other) && self.within_vertical_bounds_of(other)
    }
}

/// CSS box model — the four nested rectangles around an element plus
/// its intrinsic dimensions. Returned by [`Element::box_model`](crate::Element::box_model).
#[derive(Debug, Clone, PartialEq)]
pub struct BoxModel {
    /// Inner content quad.
    pub content: ElementQuad,
    /// Content + padding quad.
    pub padding: ElementQuad,
    /// Padding + border quad.
    pub border: ElementQuad,
    /// Border + margin quad.
    pub margin: ElementQuad,
    /// Element's intrinsic width (`offsetWidth`).
    pub width: u32,
    /// Element's intrinsic height (`offsetHeight`).
    pub height: u32,
}

impl BoxModel {
    fn quad_to_bbox(q: &ElementQuad) -> BoundingBox {
        let xs = [q.top_left.x, q.top_right.x, q.bottom_right.x, q.bottom_left.x];
        let ys = [q.top_left.y, q.top_right.y, q.bottom_right.y, q.bottom_left.y];
        let min_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_y = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        BoundingBox { x: min_x, y: min_y, width: max_x - min_x, height: max_y - min_y }
    }

    /// Bounding rectangle of the content box.
    pub fn content_rect(&self) -> BoundingBox {
        Self::quad_to_bbox(&self.content)
    }
    /// Bounding rectangle of the padding box.
    pub fn padding_rect(&self) -> BoundingBox {
        Self::quad_to_bbox(&self.padding)
    }
    /// Bounding rectangle of the border box.
    pub fn border_rect(&self) -> BoundingBox {
        Self::quad_to_bbox(&self.border)
    }
    /// Bounding rectangle of the margin box.
    pub fn margin_rect(&self) -> BoundingBox {
        Self::quad_to_bbox(&self.margin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn point_arithmetic() {
        let a = Point::new(1.0, 2.0);
        let b = Point::new(3.0, 4.0);
        assert_eq!(a + b, Point::new(4.0, 6.0));
        assert_eq!(b - a, Point::new(2.0, 2.0));
        assert_eq!(a / 2.0, Point::new(0.5, 1.0));
    }

    #[test]
    fn bounding_box_center() {
        let bb = BoundingBox::new(10.0, 20.0, 100.0, 80.0);
        assert_eq!(bb.center(), Point::new(60.0, 60.0));
    }

    #[test]
    fn bounding_box_from_quad_square() {
        let quad = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        let bb = BoundingBox::from_quad(&quad).expect("valid quad");
        assert!(approx_eq(bb.x, 0.0));
        assert!(approx_eq(bb.y, 0.0));
        assert!(approx_eq(bb.width, 10.0));
        assert!(approx_eq(bb.height, 10.0));
    }

    #[test]
    fn bounding_box_from_quad_rotated() {
        // Diamond: min/max derived across all vertices.
        let quad = [5.0, 0.0, 10.0, 5.0, 5.0, 10.0, 0.0, 5.0];
        let bb = BoundingBox::from_quad(&quad).expect("valid quad");
        assert!(approx_eq(bb.x, 0.0));
        assert!(approx_eq(bb.width, 10.0));
    }

    #[test]
    fn bounding_box_from_quad_too_short() {
        let quad = [0.0; 7];
        assert!(BoundingBox::from_quad(&quad).is_none());
    }

    #[test]
    fn viewport_default_and_new() {
        let v = Viewport::default();
        assert_eq!(v.width, 1280);
        assert_eq!(v.height, 720);
        assert!(!v.emulating_mobile);
        let v2 = Viewport::new(640, 480);
        assert_eq!(v2.width, 640);
        assert_eq!(v2.height, 480);
        assert!(approx_eq(v2.device_scale_factor, 1.0));
    }

    fn unit_quad() -> ElementQuad {
        ElementQuad::from_raw(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0]).unwrap()
    }

    #[test]
    fn element_quad_center_area_dims() {
        let q = unit_quad();
        assert_eq!(q.quad_center(), Point::new(5.0, 5.0));
        assert!(approx_eq(q.quad_area(), 100.0));
        assert!(approx_eq(q.width(), 10.0));
        assert!(approx_eq(q.height(), 10.0));
        assert!(approx_eq(q.aspect_ratio(), 1.0));
    }

    #[test]
    fn element_quad_extents() {
        let q = unit_quad();
        assert!(approx_eq(q.most_left(), 0.0));
        assert!(approx_eq(q.most_right(), 10.0));
        assert!(approx_eq(q.most_top(), 0.0));
        assert!(approx_eq(q.most_bottom(), 10.0));
    }

    #[test]
    fn element_quad_from_raw_too_short() {
        assert!(ElementQuad::from_raw(&[0.0; 7]).is_none());
    }

    #[test]
    fn element_quad_spatial_relations() {
        let upper = ElementQuad::from_raw(&[0.0, 0.0, 10.0, 0.0, 10.0, 5.0, 0.0, 5.0]).unwrap();
        let lower = ElementQuad::from_raw(&[0.0, 10.0, 10.0, 10.0, 10.0, 20.0, 0.0, 20.0]).unwrap();
        assert!(upper.strictly_above(&lower));
        assert!(upper.above(&lower));
        assert!(lower.strictly_below(&upper));
        assert!(lower.below(&upper));
        assert!(!upper.strictly_below(&lower));

        let left = ElementQuad::from_raw(&[0.0, 0.0, 5.0, 0.0, 5.0, 10.0, 0.0, 10.0]).unwrap();
        let right = ElementQuad::from_raw(&[10.0, 0.0, 20.0, 0.0, 20.0, 10.0, 10.0, 10.0]).unwrap();
        assert!(left.strictly_left_of(&right));
        assert!(right.strictly_right_of(&left));
    }

    #[test]
    fn element_quad_containment() {
        let outer =
            ElementQuad::from_raw(&[0.0, 0.0, 100.0, 0.0, 100.0, 100.0, 0.0, 100.0]).unwrap();
        let inner =
            ElementQuad::from_raw(&[10.0, 10.0, 50.0, 10.0, 50.0, 50.0, 10.0, 50.0]).unwrap();
        assert!(inner.within_bounds_of(&outer));
        assert!(inner.within_horizontal_bounds_of(&outer));
        assert!(inner.within_vertical_bounds_of(&outer));
        assert!(!outer.within_bounds_of(&inner));
    }

    #[test]
    fn box_model_content_viewport_is_bounding_rect() {
        let q = ElementQuad::from_raw(&[1.0, 2.0, 11.0, 2.0, 11.0, 12.0, 1.0, 12.0]).unwrap();
        let bm = BoxModel {
            content: q.clone(),
            padding: q.clone(),
            border: q.clone(),
            margin: q.clone(),
            width: 10,
            height: 10,
        };
        let v = bm.content_rect();
        assert!(approx_eq(v.x, 1.0));
        assert!(approx_eq(v.y, 2.0));
        assert!(approx_eq(v.width, 10.0));
        assert!(approx_eq(v.height, 10.0));
    }
}
