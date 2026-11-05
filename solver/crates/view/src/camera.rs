//! Cameras. Most are fitted to points, so a caller says what to show and from which
//! direction, and never computes an eye position.

pub type V3 = [f64; 3];

pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub(crate) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn norm(a: V3) -> V3 {
    scale(a, 1.0 / dot(a, a).sqrt().max(1e-12))
}

/// Minecraft's look vector for a yaw and pitch, in degrees.
pub fn look_vector(yaw: f64, pitch: f64) -> V3 {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()]
}

#[derive(Debug, Clone)]
enum Pose {
    Eye { eye: V3, fwd: V3 },
    Fit { points: Vec<V3>, fwd: V3 },
}

/// What to look at and how. Resolved against an image size by [`crate::Scene::frame`].
#[derive(Debug, Clone)]
pub struct Camera {
    pose: Pose,
    /// Vertical field of view, degrees.
    fov: f64,
    ortho: bool,
    /// Blocks shown top to bottom, for an orthographic camera with a fixed eye.
    ortho_height: f64,
    margin: f64,
}

impl Camera {
    fn with(pose: Pose, ortho: bool) -> Camera {
        Camera {
            pose,
            fov: 60.0,
            ortho,
            ortho_height: 64.0,
            margin: 48.0,
        }
    }

    /// From `eye`, looking at `target`.
    pub fn look_at(eye: V3, target: V3) -> Camera {
        Camera::with(
            Pose::Eye {
                eye,
                fwd: norm(sub(target, eye)),
            },
            false,
        )
    }

    /// A player's view: an eye position and a yaw and pitch, in degrees.
    pub fn mc(eye: V3, yaw: f64, pitch: f64) -> Camera {
        Camera::with(
            Pose::Eye {
                eye,
                fwd: look_vector(yaw, pitch),
            },
            false,
        )
    }

    /// Around `target` at `distance`, looking with this yaw and pitch.
    pub fn orbit(target: V3, yaw: f64, pitch: f64, distance: f64) -> Camera {
        let fwd = look_vector(yaw, pitch);
        Camera::with(
            Pose::Eye {
                eye: sub(target, scale(fwd, distance)),
                fwd,
            },
            false,
        )
    }

    /// Fits the points in frame, by default from the south and above. Set the direction
    /// with [`Camera::from`].
    pub fn fit(points: &[V3]) -> Camera {
        Camera::with(
            Pose::Fit {
                points: points.to_vec(),
                fwd: look_vector(180.0, 35.0),
            },
            false,
        )
    }

    /// Orthographic from above, north up, +x right. Set rulers with `tools::rulers`.
    pub fn top(points: &[V3]) -> Camera {
        Camera::with(
            Pose::Fit {
                points: points.to_vec(),
                fwd: [0.0, -1.0, 0.0],
            },
            true,
        )
    }

    /// Orthographic from the side, looking along `yaw` (degrees), +y up. Yaw 0 looks
    /// south, so +x is on the left; yaw 180 looks north, so +x is on the right.
    pub fn side(points: &[V3], yaw: f64) -> Camera {
        Camera::with(
            Pose::Fit {
                points: points.to_vec(),
                fwd: look_vector(yaw, 0.0),
            },
            true,
        )
    }

    /// The direction a fitted camera looks along.
    pub fn from(mut self, yaw: f64, pitch: f64) -> Camera {
        if let Pose::Fit { fwd, .. } = &mut self.pose {
            *fwd = look_vector(yaw, pitch);
        }
        self
    }

    pub fn fov(mut self, degrees: f64) -> Camera {
        self.fov = degrees;
        self
    }

    /// Orthographic.
    pub fn ortho(mut self) -> Camera {
        self.ortho = true;
        self
    }

    /// Orthographic with a fixed eye: this many blocks top to bottom.
    pub fn ortho_height(mut self, blocks: f64) -> Camera {
        self.ortho = true;
        self.ortho_height = blocks;
        self
    }

    /// Perspective.
    pub fn perspective(mut self) -> Camera {
        self.ortho = false;
        self
    }

    /// Pixels kept free around fitted points.
    pub fn margin(mut self, px: f64) -> Camera {
        self.margin = px;
        self
    }

    pub(crate) fn resolve(&self, w: u32, h: u32, world: (V3, V3)) -> View {
        let (wf, hf) = (w as f64, h as f64);
        let (cx, cy) = (wf * 0.5, hf * 0.5);
        let basis = |fwd: V3| -> (V3, V3) {
            // straight down: north is up on screen, like a map
            let seed = if fwd[1].abs() > 0.999 {
                [0.0, 0.0, -fwd[1].signum() * -1.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            let right = norm(cross(fwd, seed));
            (right, norm(cross(right, fwd)))
        };
        let focal = hf * 0.5 / (self.fov.to_radians() * 0.5).tan();
        // an eye far enough back that everything is in front of an orthographic camera
        let reach = |at: V3, fwd: V3, points: &[V3]| -> f64 {
            let mut back: f64 = 1.0;
            for x in [world.0[0], world.1[0]] {
                for y in [world.0[1], world.1[1]] {
                    for z in [world.0[2], world.1[2]] {
                        back = back.max(-dot(sub([x, y, z], at), fwd));
                    }
                }
            }
            for p in points {
                back = back.max(-dot(sub(*p, at), fwd));
            }
            back + 8.0
        };
        match &self.pose {
            Pose::Eye { eye, fwd } => {
                let (right, up) = basis(*fwd);
                if self.ortho {
                    let scale_px = hf / self.ortho_height;
                    let back = reach(*eye, *fwd, &[]);
                    View {
                        eye: sub(*eye, scale(*fwd, back)),
                        right,
                        up,
                        fwd: *fwd,
                        ortho: Some(scale_px),
                        focal,
                        cx,
                        cy,
                        w,
                        h,
                    }
                } else {
                    View {
                        eye: *eye,
                        right,
                        up,
                        fwd: *fwd,
                        ortho: None,
                        focal,
                        cx,
                        cy,
                        w,
                        h,
                    }
                }
            }
            Pose::Fit { points, fwd } => {
                let (right, up) = basis(*fwd);
                let pts: &[V3] = if points.is_empty() {
                    &[world.0, world.1]
                } else {
                    points
                };
                let (mut r0, mut r1, mut u0, mut u1) = (
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                );
                for p in pts {
                    let (r, u) = (dot(*p, right), dot(*p, up));
                    r0 = r0.min(r);
                    r1 = r1.max(r);
                    u0 = u0.min(u);
                    u1 = u1.max(u);
                }
                let (avail_w, avail_h) = (
                    (wf - 2.0 * self.margin).max(16.0),
                    (hf - 2.0 * self.margin).max(16.0),
                );
                let f_mean = {
                    let mut c = [0.0; 3];
                    for p in pts {
                        c = add(c, *p);
                    }
                    scale(c, 1.0 / pts.len() as f64)
                };
                let center = add(
                    add(
                        scale(right, (r0 + r1) * 0.5),
                        scale(up, (u0 + u1) * 0.5),
                    ),
                    scale(*fwd, dot(f_mean, *fwd)),
                );
                if self.ortho {
                    let scale_px = (avail_w / (r1 - r0).max(1.0)).min(avail_h / (u1 - u0).max(1.0));
                    let back = reach(center, *fwd, pts);
                    View {
                        eye: sub(center, scale(*fwd, back)),
                        right,
                        up,
                        fwd: *fwd,
                        ortho: Some(scale_px),
                        focal,
                        cx,
                        cy,
                        w,
                        h,
                    }
                } else {
                    // back away until every point is inside the margins
                    let mut dist: f64 = 1.0;
                    for p in pts {
                        let v = sub(*p, center);
                        let (x, y, z) = (dot(v, right), dot(v, up), dot(v, *fwd));
                        dist = dist
                            .max(x.abs() * focal / (avail_w * 0.5) - z)
                            .max(y.abs() * focal / (avail_h * 0.5) - z);
                    }
                    View {
                        eye: sub(center, scale(*fwd, dist)),
                        right,
                        up,
                        fwd: *fwd,
                        ortho: None,
                        focal,
                        cx,
                        cy,
                        w,
                        h,
                    }
                }
            }
        }
    }
}

/// A camera resolved against an image size.
#[derive(Debug, Clone)]
pub struct View {
    pub eye: V3,
    pub right: V3,
    pub up: V3,
    pub fwd: V3,
    /// Pixels per block, if orthographic.
    pub ortho: Option<f64>,
    pub focal: f64,
    pub cx: f64,
    pub cy: f64,
    pub w: u32,
    pub h: u32,
}

pub(crate) const NEAR: f64 = 0.05;

impl View {
    /// Pixel x, pixel y and depth along the view axis; None behind the near plane.
    pub fn project(&self, p: V3) -> Option<[f64; 3]> {
        let v = sub(p, self.eye);
        let z = dot(v, self.fwd);
        if z < NEAR {
            return None;
        }
        let (x, y) = (dot(v, self.right), dot(v, self.up));
        let k = self.ortho.unwrap_or(self.focal / z);
        Some([self.cx + x * k, self.cy - y * k, z])
    }

    /// Pixels per block at a depth.
    pub fn px_per_block(&self, z: f64) -> f64 {
        self.ortho.unwrap_or(self.focal / z.max(NEAR))
    }

    /// The ray through a pixel: (origin, unit direction).
    pub fn ray(&self, x: f64, y: f64) -> (V3, V3) {
        match self.ortho {
            Some(k) => (
                add(
                    self.eye,
                    add(
                        scale(self.right, (x - self.cx) / k),
                        scale(self.up, (self.cy - y) / k),
                    ),
                ),
                self.fwd,
            ),
            None => (
                self.eye,
                norm(add(
                    self.fwd,
                    add(
                        scale(self.right, (x - self.cx) / self.focal),
                        scale(self.up, (self.cy - y) / self.focal),
                    ),
                )),
            ),
        }
    }
}
