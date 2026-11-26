use golf_sim_base::{Material, Real, Vec2, Vec4, fm, num::lerpf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitHeight {
    pub datum_m: Real,
    pub offset_m: Real,
    pub slope: Vec2,
}

impl SplitHeight {
    pub fn height(self) -> Real {
        self.datum_m + self.offset_m
    }

    pub(crate) fn blend(self, near: Self, depth: Real, width: Real, inward: Vec2) -> Self {
        let t = depth / width;
        let weight = t * t * (3.0 - 2.0 * t);
        let gap = near.datum_m - self.datum_m + near.offset_m - self.offset_m;
        // The changing blend weight contributes its own slope normal to the edge.
        let ramp = gap * 6.0 * t * (1.0 - t) / width;
        Self {
            datum_m: self.datum_m,
            offset_m: self.offset_m + gap * weight,
            slope: Vec2::new(
                lerpf(self.slope.x, near.slope.x, weight) + ramp * inward.x,
                lerpf(self.slope.y, near.slope.y, weight) + ramp * inward.y,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridFrame {
    pub origin: Vec2,
    pub cell_m: Real,
    pub size: [usize; 2],
    span_end: Vec2,
}

impl GridFrame {
    pub(crate) fn new(origin: Vec2, cell_m: Real, size: [usize; 2], span_end: Vec2) -> Self {
        Self {
            origin,
            cell_m,
            size,
            span_end,
        }
    }

    pub fn span_end(&self) -> Vec2 {
        self.span_end
    }

    pub fn contains(&self, p: Vec2) -> bool {
        let end = self.span_end();
        p.x >= self.origin.x && p.x < end.x && p.y >= self.origin.y && p.y < end.y
    }

    fn coordinates(&self, p: Vec2) -> Vec2 {
        (p - self.origin) / self.cell_m
    }
}

#[derive(Debug, PartialEq)]
pub struct HeightGrid {
    frame: GridFrame,
    heights: Vec<Real>,
    inner_start: Vec2,
    inner_end: Vec2,
}

impl HeightGrid {
    pub(crate) fn new(frame: GridFrame, heights: Vec<Real>, inner: [Vec2; 2]) -> Self {
        Self {
            frame,
            heights,
            inner_start: inner[0],
            inner_end: inner[1],
        }
    }

    pub fn frame(&self) -> &GridFrame {
        &self.frame
    }

    pub fn heights(&self) -> &[Real] {
        &self.heights
    }

    fn edge_distances(&self, p: Vec2) -> [Real; 4] {
        [
            p.x - self.inner_start.x,
            self.inner_end.x - p.x,
            p.y - self.inner_start.y,
            self.inner_end.y - p.y,
        ]
    }

    pub fn inner_distance(&self, p: Vec2) -> Real {
        let [left, right, top, bottom] = self.edge_distances(p);
        left.min(right).min(top.min(bottom))
    }

    pub(crate) fn inner_direction(&self, p: Vec2) -> Vec2 {
        let [left, right, top, bottom] = self.edge_distances(p);
        let nearest = left.min(right).min(top.min(bottom));
        if nearest == left {
            Vec2::new(1.0, 0.0)
        } else if nearest == right {
            Vec2::new(-1.0, 0.0)
        } else if nearest == top {
            Vec2::new(0.0, 1.0)
        } else {
            Vec2::new(0.0, -1.0)
        }
    }

    pub fn sample_split(&self, p: Vec2) -> SplitHeight {
        let grid = self.frame.coordinates(p);
        let [width, height] = self.frame.size;
        let i = (fm::floor(grid.x) as usize).clamp(1, width - 3);
        let j = (fm::floor(grid.y) as usize).clamp(1, height - 3);
        let (wx, dx) = weights((grid.x - i as Real).clamp(0.0, 1.0));
        let (wz, dz) = weights((grid.y - j as Real).clamp(0.0, 1.0));

        // Subtract a node before taking weighted sums to retain small changes in elevation.
        let datum = self.heights[j * width + i];
        let base = (j - 1) * width + i - 1;
        let row = |offset: usize| {
            let start = base + offset * width;
            let h = &self.heights[start..start + 4];
            Vec4::new(h[0] - datum, h[1] - datum, h[2] - datum, h[3] - datum)
        };
        let [a, b, c, d] = [row(0), row(1), row(2), row(3)];
        let along = Vec4::new(a.dot(wx), b.dot(wx), c.dot(wx), d.dot(wx));
        let across = Vec4::new(a.dot(dx), b.dot(dx), c.dot(dx), d.dot(dx));
        SplitHeight {
            datum_m: datum,
            offset_m: wz.dot(along),
            slope: Vec2::new(
                wz.dot(across) / self.frame.cell_m,
                dz.dot(along) / self.frame.cell_m,
            ),
        }
    }
}

fn weights(t: Real) -> (Vec4, Vec4) {
    let t2 = t * t;
    let t3 = t2 * t;
    let value = Vec4::new(
        -t3 + 2.0 * t2 - t,
        3.0 * t3 - 5.0 * t2 + 2.0,
        -3.0 * t3 + 4.0 * t2 + t,
        t3 - t2,
    );
    let derivative = Vec4::new(
        -3.0 * t2 + 4.0 * t - 1.0,
        9.0 * t2 - 10.0 * t,
        -9.0 * t2 + 8.0 * t + 1.0,
        3.0 * t2 - 2.0 * t,
    );
    (value.scale(0.5), derivative.scale(0.5))
}

#[derive(Debug, PartialEq)]
pub struct SurfaceGrid {
    frame: GridFrame,
    materials: Vec<u8>,
    stand: Vec<u8>,
}

impl SurfaceGrid {
    pub(crate) fn new(frame: GridFrame, materials: Vec<u8>, stand: Vec<u8>) -> Self {
        Self {
            frame,
            materials,
            stand,
        }
    }

    pub fn frame(&self) -> &GridFrame {
        &self.frame
    }

    pub fn material_at(&self, p: Vec2) -> Material {
        Material(self.materials[self.nearest_node(p)])
    }

    pub fn stand_at(&self, p: Vec2) -> Real {
        if self.stand.is_empty() {
            1.0
        } else {
            Real::from(self.stand[self.nearest_node(p)]) / 255.0
        }
    }

    fn nearest_node(&self, p: Vec2) -> usize {
        let grid = self.frame.coordinates(p);
        let [width, height] = self.frame.size;
        let i = (fm::round(grid.x) as usize).min(width - 1);
        let j = (fm::round(grid.y) as usize).min(height - 1);
        j * width + i
    }
}
