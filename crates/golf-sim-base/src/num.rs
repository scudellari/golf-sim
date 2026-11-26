//! Godot-order vector operations and scalar built-ins.

use std::{
    f64::consts::{PI, TAU},
    ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign},
};

use crate::{Basis, Real, Vec2, Vec3, Vec4, fm};

const CMP_EPSILON: Real = 0.00001;

pub fn lerpf(from: Real, to: Real, weight: Real) -> Real {
    from + (to - from) * weight
}

pub fn clampf(value: Real, min: Real, max: Real) -> Real {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

fn is_equal_approx(a: Real, b: Real) -> bool {
    a == b || (a - b).abs() < (CMP_EPSILON * a.abs()).max(CMP_EPSILON)
}

pub fn smoothstep(from: Real, to: Real, value: Real) -> Real {
    if is_equal_approx(from, to) {
        if from <= to {
            return if value <= from { 0.0 } else { 1.0 };
        }
        return if value <= to { 1.0 } else { 0.0 };
    }
    let t = clampf(inverse_lerp(from, to, value), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn inverse_lerp(from: Real, to: Real, value: Real) -> Real {
    (value - from) / (to - from)
}

pub fn lerp_angle(from: Real, to: Real, weight: Real) -> Real {
    let difference = fm::fmod(to - from, TAU);
    let distance = fm::fmod(2.0 * difference, TAU) - difference;
    from + distance * weight
}

pub fn fposmod(value: Real, modulus: Real) -> Real {
    let mut remainder = fm::fmod(value, modulus);
    if (remainder < 0.0 && modulus > 0.0) || (remainder > 0.0 && modulus < 0.0) {
        remainder += modulus;
    }
    // Godot returns positive zero for either sign of zero.
    remainder + 0.0
}

/// A nonzero remainder takes the sign of the modulus. Panics if the modulus is zero.
pub fn posmod(value: i64, modulus: i64) -> i64 {
    if modulus == -1 {
        return 0;
    }
    let remainder = value % modulus;
    if (remainder < 0 && modulus > 0) || (remainder > 0 && modulus < 0) {
        remainder + modulus
    } else {
        remainder
    }
}

pub fn wrapf(value: Real, min: Real, max: Real) -> Real {
    let range = max - min;
    if range.abs() < CMP_EPSILON {
        return min;
    }
    let result = value - range * fm::floor((value - min) / range);
    if is_equal_approx(result, max) {
        min
    } else {
        result
    }
}

pub fn snappedf(value: Real, step: Real) -> Real {
    if step == 0.0 {
        value
    } else {
        fm::floor(value / step + 0.5) * step
    }
}

pub fn deg_to_rad(degrees: Real) -> Real {
    degrees * (PI / 180.0)
}

pub fn rad_to_deg(radians: Real) -> Real {
    radians * (180.0 / PI)
}

pub fn roundf(value: Real) -> Real {
    fm::round(value)
}

pub fn roundi(value: Real) -> i64 {
    roundf(value) as i64
}

impl Vec2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);

    pub const fn new(x: Real, y: Real) -> Self {
        Self { x, y }
    }

    pub fn dot(self, other: Self) -> Real {
        self.x * other.x + self.y * other.y
    }

    pub fn cross(self, other: Self) -> Real {
        self.x * other.y - self.y * other.x
    }

    pub fn length_squared(self) -> Real {
        self.x * self.x + self.y * self.y
    }

    pub fn length(self) -> Real {
        fm::sqrt(self.length_squared())
    }

    pub fn normalized(self) -> Self {
        let squared = self.length_squared();
        if squared != 0.0 {
            self / fm::sqrt(squared)
        } else {
            self
        }
    }

    pub fn lerp(self, to: Self, weight: Real) -> Self {
        Self::new(lerpf(self.x, to.x, weight), lerpf(self.y, to.y, weight))
    }

    pub fn slide(self, normal: Self) -> Self {
        self - normal * self.dot(normal)
    }

    pub fn limit_length(self, limit: Real) -> Self {
        let length = self.length();
        if length > 0.0 && limit < length {
            (self / length) * limit
        } else {
            self
        }
    }

    pub fn distance_to(self, other: Self) -> Real {
        (self - other).length()
    }

    pub fn rotated(self, angle: Real) -> Self {
        let sine = fm::sin(angle);
        let cosine = fm::cos(angle);
        Self::new(
            self.x * cosine - self.y * sine,
            self.x * sine + self.y * cosine,
        )
    }

    pub fn from_angle(angle: Real) -> Self {
        Self::new(fm::cos(angle), fm::sin(angle))
    }

    pub fn angle_to(self, other: Self) -> Real {
        fm::atan2(self.cross(other), self.dot(other))
    }
}

impl Sub for Vec2 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl Mul<Real> for Vec2 {
    type Output = Self;

    fn mul(self, factor: Real) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
}

impl Div<Real> for Vec2 {
    type Output = Self;

    fn div(self, divisor: Real) -> Self {
        Self::new(self.x / divisor, self.y / divisor)
    }
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: Real, y: Real, z: Real) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, other: Self) -> Real {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn length_squared(self) -> Real {
        let x2 = self.x * self.x;
        let y2 = self.y * self.y;
        let z2 = self.z * self.z;
        x2 + y2 + z2
    }

    pub fn length(self) -> Real {
        fm::sqrt(self.length_squared())
    }

    pub fn normalized(self) -> Self {
        let squared = self.length_squared();
        if squared == 0.0 {
            Self::ZERO
        } else {
            self / fm::sqrt(squared)
        }
    }

    pub fn lerp(self, to: Self, weight: Real) -> Self {
        Self::new(
            lerpf(self.x, to.x, weight),
            lerpf(self.y, to.y, weight),
            lerpf(self.z, to.z, weight),
        )
    }

    pub fn slide(self, normal: Self) -> Self {
        self - normal * self.dot(normal)
    }

    pub fn limit_length(self, limit: Real) -> Self {
        let length = self.length();
        if length > 0.0 && limit < length {
            (self / length) * limit
        } else {
            self
        }
    }

    pub fn distance_to(self, other: Self) -> Real {
        (other - self).length()
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    pub fn cdiv(self, other: Self) -> Self {
        Self::new(self.x / other.x, self.y / other.y, self.z / other.z)
    }

    pub fn rotated(self, axis: Self, angle: Real) -> Self {
        Basis::from_axis_angle(axis, angle).xform(self)
    }

    pub fn angle_to(self, other: Self) -> Real {
        fm::atan2(self.cross(other).length(), self.dot(other))
    }

    pub fn signed_angle_to(self, other: Self, axis: Self) -> Real {
        let cross = self.cross(other);
        let angle = fm::atan2(cross.length(), self.dot(other));
        if cross.dot(axis) < 0.0 { -angle } else { angle }
    }
}

impl Add for Vec3 {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Neg for Vec3 {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<Real> for Vec3 {
    type Output = Self;

    fn mul(self, factor: Real) -> Self {
        Self::new(self.x * factor, self.y * factor, self.z * factor)
    }
}

impl Div<Real> for Vec3 {
    type Output = Self;

    fn div(self, divisor: Real) -> Self {
        Self::new(self.x / divisor, self.y / divisor, self.z / divisor)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl SubAssign for Vec3 {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

impl Vec4 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(x: Real, y: Real, z: Real, w: Real) -> Self {
        Self { x, y, z, w }
    }

    pub fn dot(self, other: Self) -> Real {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    pub fn length_squared(self) -> Real {
        self.dot(self)
    }

    pub fn length(self) -> Real {
        fm::sqrt(self.length_squared())
    }

    pub fn normalized(self) -> Self {
        let squared = self.length_squared();
        if squared == 0.0 {
            Self::ZERO
        } else {
            let length = fm::sqrt(squared);
            Self::new(
                self.x / length,
                self.y / length,
                self.z / length,
                self.w / length,
            )
        }
    }

    pub fn lerp(self, to: Self, weight: Real) -> Self {
        Self::new(
            lerpf(self.x, to.x, weight),
            lerpf(self.y, to.y, weight),
            lerpf(self.z, to.z, weight),
            lerpf(self.w, to.w, weight),
        )
    }

    pub fn scale(self, factor: Real) -> Self {
        Self::new(
            self.x * factor,
            self.y * factor,
            self.z * factor,
            self.w * factor,
        )
    }
}

impl Sub for Vec4 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(
            self.x - other.x,
            self.y - other.y,
            self.z - other.z,
            self.w - other.w,
        )
    }
}

impl Basis {
    /// Builds a rotation from a unit axis and an angle in radians.
    pub fn from_axis_angle(axis: Vec3, angle: Real) -> Self {
        let Vec3 { x, y, z } = axis;
        let xx = x * x;
        let yy = y * y;
        let zz = z * z;
        let cosine = fm::cos(angle);
        let m00 = xx + cosine * (1.0 - xx);
        let m11 = yy + cosine * (1.0 - yy);
        let m22 = zz + cosine * (1.0 - zz);
        let sine = fm::sin(angle);
        let t = 1.0 - cosine;
        let xyt = x * y * t;
        let zs = z * sine;
        let xzt = x * z * t;
        let ys = y * sine;
        let yzt = y * z * t;
        let xs = x * sine;
        Self {
            rows: [
                Vec3::new(m00, xyt - zs, xzt + ys),
                Vec3::new(xyt + zs, m11, yzt - xs),
                Vec3::new(xzt - ys, yzt + xs, m22),
            ],
        }
    }

    pub fn xform(self, vector: Vec3) -> Vec3 {
        let [x, y, z] = self.rows;
        Vec3::new(x.dot(vector), y.dot(vector), z.dot(vector))
    }

    /// Inverts a basis with nonzero determinant.
    pub fn inverse(self) -> Self {
        let [a, b, c] = self.rows;
        let co = b.cross(c);
        let scale = 1.0 / a.dot(co);
        Self {
            rows: [
                Vec3::new(co.x, a.z * c.y - a.y * c.z, a.y * b.z - a.z * b.y) * scale,
                Vec3::new(co.y, a.x * c.z - a.z * c.x, a.z * b.x - a.x * b.z) * scale,
                Vec3::new(co.z, a.y * c.x - a.x * c.y, a.x * b.y - a.y * b.x) * scale,
            ],
        }
    }
}
