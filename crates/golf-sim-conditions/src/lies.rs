//! A ball's stored lie and the ground under its address.

use golf_sim_base::{Real, Seed, Surface, Vec2, Vec3, fm, seed::SeedStream};
use golf_sim_course::legal::StanceFootprint;
use golf_sim_physics::{samplers::GroundSampler, values::Lie};

const MIN_SPAN_M: Real = 0.05;

// Fine fescue has the leaf density of mown rough. Its leaves stand 150–300 mm
// (Purdue Extension TURF-67-W), with thin seed stalks above them.
const LEAF_HEIGHT_M: &[(Surface, Real)] = &[(Surface::DeepRough, 0.225)];

fn sit_profile(surface: Surface) -> ([Real; 3], Real, Real) {
    match surface {
        Surface::Fairway => ([8.0, 5.0, 2.0], 12.0, 0.3),
        Surface::FirstCut => ([12.0, 7.0, 3.0], 25.0, 0.4),
        Surface::Rough => ([25.0, 10.0, 2.0], 55.0, 0.55),
        Surface::DeepRough => ([30.0, 8.0, 0.0], 100.0, 0.55),
        Surface::Fringe => ([5.0, 4.0, 2.0], 10.0, 0.3),
        Surface::Green => ([2.0; 3], 3.0, 0.2),
        Surface::Bunker => ([0.0, -2.0, -12.0], 0.0, 0.0),
        Surface::Tee | Surface::Hardpan | Surface::CartPath | Surface::Water => {
            ([0.0; 3], 0.0, 0.0)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Sit {
    Up,
    Normal,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DivotLie {
    None,
    Fresh,
    Old,
    SandFilled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BallLie {
    pub spot: Vec3,
    pub surface: Surface,
    pub sit: Sit,
    pub ball_height_m: Real,
    pub grass_height_m: Real,
    pub grass_density: Real,
    pub sand_firmness: Real,
    pub divot: DivotLie,
}

impl BallLie {
    pub fn on(surface: Surface, sit: Sit, spot: Vec3) -> Self {
        let (ball_mm, grass_mm, _) = sit_profile(surface);
        let mut lie = Self {
            spot,
            surface,
            sit,
            ball_height_m: ball_mm[sit as usize] / 1000.0,
            grass_height_m: 0.0,
            grass_density: 0.0,
            sand_firmness: 0.5,
            divot: DivotLie::None,
        };
        lie.set_grass(grass_mm / 1000.0);
        lie
    }

    pub fn set_grass(&mut self, height_m: Real) {
        self.grass_height_m = height_m;
        self.grass_density = sit_profile(self.surface).2;
        if let Some(&(_, leaf_m)) = LEAF_HEIGHT_M
            .iter()
            .find(|&&(surface, leaf_m)| surface == self.surface && height_m > leaf_m)
        {
            self.grass_density *= leaf_m / height_m;
        }
    }

    pub(crate) fn roll_sit(surface: Surface, seed: Seed) -> (Sit, Option<Real>) {
        let (up, down) = match surface {
            Surface::Fairway => (0.25, 0.10),
            Surface::FirstCut => (0.25, 0.20),
            Surface::Rough => (0.20, 0.35),
            Surface::DeepRough => (0.10, 0.55),
            Surface::Bunker => (0.20, 0.15),
            _ => return (Sit::Normal, None),
        };
        let draw = SeedStream::new(seed).next_float();
        let sit = if draw < up {
            Sit::Up
        } else if draw >= 1.0 - down {
            Sit::Down
        } else {
            Sit::Normal
        };
        (sit, Some(draw))
    }

    pub fn is_teed(&self) -> bool {
        self.surface == Surface::Tee
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_leaf_stand_contributes_to_fescue_density() {
        let mut fescue = BallLie::on(Surface::DeepRough, Sit::Normal, Vec3::ZERO);
        assert_eq!(fescue.grass_height_m, 0.1);
        assert_eq!(fescue.grass_density, 0.55);
        fescue.set_grass(0.42);
        assert_eq!(fescue.grass_height_m, 0.42);
        assert!((fescue.grass_density - 0.55 * 0.225 / 0.42).abs() < 1e-12);

        let mut rough = BallLie::on(Surface::Rough, Sit::Normal, Vec3::ZERO);
        for height_m in [0.0, 0.055, 0.42] {
            rough.set_grass(height_m);
            assert_eq!(rough.grass_density, 0.55);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AddressLie {
    pub ball: BallLie,
    pub slope_along_rad: Real,
    pub slope_across_rad: Real,
    pub ball_slope_along_rad: Real,
    pub lead_foot_surface: Surface,
    pub trail_foot_surface: Surface,
    pub club_entry_surface: Surface,
}

impl AddressLie {
    pub(crate) fn sample(
        ground: &impl GroundSampler,
        ball: BallLie,
        footprint: &StanceFootprint,
        aim: Vec3,
    ) -> Self {
        let sample = |point: Vec2| {
            let spot = StanceFootprint::to_world(point, ball.spot, aim);
            ground.sample_ground(spot.x, spot.z)
        };
        let lead = sample(footprint.lead_foot_m);
        let trail = sample(footprint.trail_foot_m);
        let entry = sample(footprint.club_entry_m);
        let along = footprint.lead_foot_m.x - footprint.trail_foot_m.x;
        let slope_along_rad = if along.abs() > MIN_SPAN_M {
            fm::atan((lead.height_m - trail.height_m) / along)
        } else {
            0.0
        };

        let under_ball = ground.sample_ground(ball.spot.x, ball.spot.z).normal;
        let forward = Vec3::new(aim.x, 0.0, aim.z).normalized();
        let ball_slope_along_rad = fm::atan(-under_ball.dot(forward) / under_ball.y);

        // Measure square to the aim so a forward ball position adds no uphill component.
        let centre = footprint.centre_m();
        let slope_across_rad = if centre.y.abs() > MIN_SPAN_M {
            let feet = sample(centre);
            let line = sample(Vec2::new(centre.x, 0.0));
            fm::atan((line.height_m - feet.height_m) / -centre.y)
        } else {
            0.0
        };
        Self {
            ball,
            slope_along_rad,
            slope_across_rad,
            ball_slope_along_rad,
            lead_foot_surface: lead.surface,
            trail_foot_surface: trail.surface,
            club_entry_surface: entry.surface,
        }
    }

    pub fn sim_lie(&self, tee_height_m: Real) -> Lie {
        let ball = self.ball;
        Lie {
            surface: ball.surface,
            ball_height_m: if ball.is_teed() {
                tee_height_m
            } else {
                ball.ball_height_m
            },
            grass_height_m: ball.grass_height_m,
            grass_density: ball.grass_density,
            sand_firmness: ball.sand_firmness,
        }
    }
}
