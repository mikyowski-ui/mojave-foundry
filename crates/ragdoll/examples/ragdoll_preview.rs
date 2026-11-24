//! Runs the ragdoll presets on a skeleton and writes side-view stick-figure
//! contact sheets (binary PPM) so a person can eyeball the falls.
//!
//! ```text
//! cargo run -p ragdoll --release --example ragdoll_preview -- \
//!     [--skeleton <skeletons/<hash>.json>] [--preset <name>|all] [--seed <n>] \
//!     [--tone <seconds>] [--jitter <0..1>] [--push <scale>] [--rest] [--out <dir>]
//! ```
//!
//! Without `--skeleton` it uses the crate's synthetic humanoid. The start
//! pose is the skeleton's rest pose with the upper arms lowered to the
//! sides (`ragdoll::fixture::stand`), or the bare rest pose with `--rest`. A skeleton
//! file is a `nif::Skeleton` document of the MIR store (read-only). The
//! output directory defaults to `out/ragdoll-scratch/`, which git ignores.
//! For each preset it prints the pelvis and head trajectory, the settle
//! time and the wall time of the simulation, and writes
//! `<label>_<preset>.ppm`: one side-view cell per sampled frame (forward to
//! the right, up to the top, the ground in brown; legs blue (left
//! lighter), arms orange (left yellow), spine and head white, other bones
//! grey), then the corpse from the side, the front and
//! above; and `<label>_<preset>_corpse.ppm`, those three views larger.

use std::path::PathBuf;
use std::time::Instant;

use nif::{Bone, BoneTransform, Pose, Skeleton};
use ragdoll::{Impulse, Preset, RagdollSpec, simulate};

fn main() {
    if let Err(e) = run() {
        eprintln!("ragdoll_preview: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut skeleton_path: Option<PathBuf> = None;
    let mut preset = "all".to_owned();
    let mut seed = 1u64;
    let mut out = PathBuf::from("out/ragdoll-scratch");
    let mut tone: Option<f32> = None;
    let mut stand = true;
    let mut scale: Option<f32> = None;
    let mut jitter: Option<f32> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--skeleton" => skeleton_path = Some(PathBuf::from(value()?)),
            "--preset" => preset = value()?,
            "--seed" => seed = value()?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--out" => out = PathBuf::from(value()?),
            "--rest" => stand = false,
            "--push" => scale = Some(value()?.parse().map_err(|e| format!("--push: {e}"))?),
            "--tone" => tone = Some(value()?.parse().map_err(|e| format!("--tone: {e}"))?),
            "--jitter" => jitter = Some(value()?.parse().map_err(|e| format!("--jitter: {e}"))?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let (label, skeleton) = match &skeleton_path {
        Some(path) => {
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("skeleton");
            (
                stem.chars().take(8).collect::<String>(),
                skeleton_from_json(&text)?,
            )
        }
        None => ("synthetic".to_owned(), ragdoll::fixture::humanoid()),
    };
    let presets: Vec<Preset> = Preset::ALL
        .into_iter()
        .filter(|p| preset == "all" || p.name() == preset)
        .collect();
    if presets.is_empty() {
        return Err(format!("no preset called {preset}"));
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let spec = RagdollSpec::humanoid();
    // The rest pose, or (by default) the rest pose with the arms lowered to
    // the sides, as a stand holds them.
    let start = if stand {
        ragdoll::fixture::stand(&skeleton)
    } else {
        Pose::rest(&skeleton)
    };
    println!("{label}: {} bones", skeleton.bones.len());
    for preset in presets {
        let mut impulse = Impulse::preset(preset);
        if let Some(t) = tone {
            impulse.tone = t;
        }
        if let Some(j) = jitter {
            impulse.jitter = j;
        }
        if let Some(k) = scale {
            for push in &mut impulse.pushes {
                push.speed *= k;
            }
        }
        let began = Instant::now();
        let clip = simulate(&skeleton, &start, &spec, &impulse, seed, 4.0, 10.0)
            .map_err(|e| format!("{}: {e}", preset.name()))?;
        let elapsed = began.elapsed();
        println!(
            "{}: {} frames, {} bodies, settled at {:.2} s ({}{}), {} steps in {:.1} ms, pelvis ends {:.1} right {:.1} forward",
            preset.name(),
            clip.poses.len(),
            clip.bodies,
            clip.settle_time,
            if clip.settled {
                "at rest"
            } else {
                "NOT at rest"
            },
            if clip.forced { ", forced" } else { "" },
            clip.steps,
            elapsed.as_secs_f64() * 1e3,
            clip.root_travel[0],
            clip.root_travel[1],
        );
        let track = |name: &str| skeleton.find(name).map(|i| i as usize);
        let (pelvis, head) = (track("Bip01 Pelvis"), track("Bip01 Head"));
        for (k, pose) in clip.poses.iter().enumerate() {
            let at = |b: Option<usize>| {
                b.map(|b| {
                    let m = pose.world[b];
                    format!("({:6.1} {:6.1} {:6.1})", m[0][3], m[1][3], m[2][3])
                })
                .unwrap_or_default()
            };
            println!("  {k:2}  pelvis {}  head {}", at(pelvis), at(head));
        }
        let corpse = clip.corpse();
        let mut line = String::from("  corpse:");
        for body in &spec.bodies {
            if let Some(i) = body.bones.iter().find_map(|n| skeleton.find(n)) {
                let m = corpse.world[i as usize];
                line += &format!(
                    " {} ({:.0} {:.0} {:.0})",
                    body.key, m[0][3], m[1][3], m[2][3]
                );
            }
        }
        // Which way the chest faces at the end: its rest forward (+Y),
        // carried along with the bone.
        if let Some(chest) = ["Bip01 Spine2", "Bip01 Spine1", "Bip01 Spine"]
            .iter()
            .find_map(|n| skeleton.find(n))
        {
            let (rest, now) = (start.world[chest as usize], corpse.world[chest as usize]);
            // f_local = R_restᵀ · (0, 1, 0); f_now = R_now · f_local.
            let local: [f32; 3] = std::array::from_fn(|c| rest[1][c]);
            let up: f32 = (0..3).map(|c| now[2][c] * local[c]).sum();
            line += &format!(
                "  chest faces {} ({up:.2})",
                if up > 0.5 {
                    "up"
                } else if up < -0.5 {
                    "down"
                } else {
                    "sideways"
                }
            );
        }
        println!("{line}");
        let sheet = contact_sheet(&skeleton, &clip.poses, 1.5, 6);
        let path = out.join(format!("{label}_{}.ppm", preset.name()));
        std::fs::write(&path, sheet).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("  wrote {}", path.display());
        // The corpse alone, larger.
        let corpse = contact_sheet(&skeleton, std::slice::from_ref(clip.corpse()), 4.0, 1);
        let path = out.join(format!("{label}_{}_corpse.ppm", preset.name()));
        std::fs::write(&path, corpse).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- drawing

struct Canvas {
    width: usize,
    height: usize,
    rgb: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Canvas {
        Canvas {
            width,
            height,
            rgb: vec![24; width * height * 3],
        }
    }

    fn dot(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let i = (y as usize * self.width + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 3], thick: bool) {
        let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as i64).max(1);
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let x = (a.0 + (b.0 - a.0) * t).round() as i64;
            let y = (a.1 + (b.1 - a.1) * t).round() as i64;
            self.dot(x, y, c);
            if thick {
                self.dot(x + 1, y, c);
                self.dot(x, y + 1, c);
            }
        }
    }

    fn ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.extend_from_slice(&self.rgb);
        out
    }
}

/// A view: which root-space axes go right and up on the picture.
#[derive(Clone, Copy)]
enum View {
    Side,
    Front,
    Top,
}

impl View {
    fn project(self, p: [f32; 3]) -> (f32, f32) {
        match self {
            View::Side => (p[1], p[2]),
            View::Front => (p[0], p[2]),
            View::Top => (p[0], p[1]),
        }
    }
}

fn contact_sheet(skeleton: &Skeleton, poses: &[Pose], scale: f32, columns: usize) -> Vec<u8> {
    // Bones drawn: every bone whose parent is not a root (that skips the
    // scene root's helpers, such as the lift from the ground to the hips).
    let edges: Vec<(usize, usize)> = skeleton
        .bones
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let p = b.parent? as usize;
            skeleton.bones[p].parent.map(|_| (p, i))
        })
        .collect();
    let simulated: Vec<bool> = {
        let spec = RagdollSpec::humanoid();
        let mut v = vec![false; skeleton.bones.len()];
        for body in &spec.bodies {
            if let Some(i) = body.bones.iter().find_map(|n| skeleton.find(n)) {
                v[i as usize] = true;
            }
        }
        v
    };
    let origin = |pose: &Pose, i: usize| {
        let m = pose.world[i];
        [m[0][3], m[1][3], m[2][3]]
    };
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for pose in poses {
        for &(_, i) in &edges {
            let o = origin(pose, i);
            for k in 0..3 {
                lo[k] = lo[k].min(o[k]);
                hi[k] = hi[k].max(o[k]);
            }
        }
    }
    let pad = 12.0;
    let cell = |view: View| {
        let (a, b) = (view.project(lo), view.project(hi));
        let lo_up = if matches!(view, View::Top) {
            a.1
        } else {
            a.1.min(0.0)
        };
        (
            ((b.0 - a.0 + 2.0 * pad) * scale).ceil() as usize,
            ((b.1 - lo_up + 2.0 * pad) * scale).ceil() as usize,
            a.0 - pad,
            b.1 + pad,
        )
    };
    let (side_w, side_h, side_x0, side_top) = cell(View::Side);
    let rows = poses.len().div_ceil(columns);
    let views = [View::Side, View::Front, View::Top];
    let finals: Vec<_> = views.iter().map(|v| cell(*v)).collect();
    let final_w: usize = finals.iter().map(|c| c.0).sum();
    let final_h = finals.iter().map(|c| c.1).max().unwrap_or(0);
    let width = (side_w * columns).max(final_w);
    let height = side_h * rows + final_h;
    let mut canvas = Canvas::new(width, height);

    let draw =
        |canvas: &mut Canvas, pose: &Pose, view: View, ox: usize, oy: usize, x0: f32, top: f32| {
            let to_px = |p: [f32; 3]| {
                let (u, v) = view.project(p);
                (ox as f32 + (u - x0) * scale, oy as f32 + (top - v) * scale)
            };
            if !matches!(view, View::Top) {
                let ground = to_px([lo[0] - 1e3, lo[1] - 1e3, 0.0]).1;
                let w = match view {
                    View::Side => side_w,
                    _ => finals[1].0,
                };
                canvas.line(
                    (ox as f32, ground),
                    ((ox + w - 1) as f32, ground),
                    [140, 90, 40],
                    false,
                );
            }
            for &(p, i) in &edges {
                let name = &skeleton.bones[i].name;
                let has = |parts: &[&str]| parts.iter().any(|p| name.contains(p));
                let left = name.contains(" L ") || name.contains(" L") && name.contains("Twist");
                let colour = if has(&["Thigh", "Calf", "Foot", "Toe"]) {
                    if left { [90, 200, 255] } else { [40, 120, 255] }
                } else if has(&["Clavicle", "Arm", "Fore", "Hand", "Finger", "Thumb"]) {
                    if left { [255, 220, 60] } else { [255, 140, 40] }
                } else if simulated[i] {
                    [235, 235, 235]
                } else {
                    [120, 120, 130]
                };
                canvas.line(
                    to_px(origin(pose, p)),
                    to_px(origin(pose, i)),
                    colour,
                    simulated[i],
                );
            }
        };
    for (k, pose) in poses.iter().enumerate() {
        let (ox, oy) = ((k % columns) * side_w, (k / columns) * side_h);
        // Cell frame.
        for x in ox..ox + side_w {
            canvas.dot(x as i64, oy as i64, [60, 60, 70]);
        }
        for y in oy..oy + side_h {
            canvas.dot(ox as i64, y as i64, [60, 60, 70]);
        }
        draw(&mut canvas, pose, View::Side, ox, oy, side_x0, side_top);
    }
    let corpse = poses.last().expect("a clip has its start pose");
    let mut ox = 0;
    for (view, c) in views.iter().zip(&finals) {
        draw(&mut canvas, corpse, *view, ox, side_h * rows, c.2, c.3);
        ox += c.0;
    }
    canvas.ppm()
}

// ------------------------------------------------------------- tiny JSON

#[derive(Debug)]
enum Json {
    Null,
    Bool,
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn num(&self) -> Option<f32> {
        match self {
            Json::Num(v) => Some(*v as f32),
            _ => None,
        }
    }
    fn arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", c as char, self.i))
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 64 {
            return Err("JSON nested too deep".into());
        }
        self.ws();
        match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut fields = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(fields));
                }
                loop {
                    self.ws();
                    let Json::Str(k) = self.value(depth + 1)? else {
                        return Err(format!("expected a key at byte {}", self.i));
                    };
                    self.eat(b':')?;
                    fields.push((k, self.value(depth + 1)?));
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(fields));
                        }
                        _ => return Err(format!("expected `,` or `}}` at byte {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(format!("expected `,` or `]` at byte {}", self.i)),
                    }
                }
            }
            Some(b'"') => {
                self.i += 1;
                let mut out = String::new();
                loop {
                    match self.s.get(self.i) {
                        None => return Err("unterminated string".into()),
                        Some(b'"') => {
                            self.i += 1;
                            return Ok(Json::Str(out));
                        }
                        Some(b'\\') => {
                            let c = *self.s.get(self.i + 1).ok_or("bad escape")?;
                            self.i += 2;
                            match c {
                                b'n' => out.push('\n'),
                                b't' => out.push('\t'),
                                b'u' => {
                                    let hex = self.s.get(self.i..self.i + 4).ok_or("bad \\u")?;
                                    let code = u32::from_str_radix(
                                        std::str::from_utf8(hex).map_err(|e| e.to_string())?,
                                        16,
                                    )
                                    .map_err(|e| e.to_string())?;
                                    out.push(char::from_u32(code).unwrap_or('?'));
                                    self.i += 4;
                                }
                                other => out.push(other as char),
                            }
                        }
                        Some(_) => {
                            let rest = std::str::from_utf8(&self.s[self.i..])
                                .map_err(|e| e.to_string())?;
                            let c = rest.chars().next().ok_or("bad string")?;
                            out.push(c);
                            self.i += c.len_utf8();
                        }
                    }
                }
            }
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Json::Bool)
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Json::Bool)
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Json::Null)
            }
            Some(_) => {
                let start = self.i;
                while self.i < self.s.len()
                    && matches!(
                        self.s[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()
                    .and_then(|t| t.parse::<f64>().ok())
                    .map(Json::Num)
                    .ok_or(format!("bad value at byte {start}"))
            }
            None => Err("unexpected end of JSON".into()),
        }
    }
}

/// Reads a `nif::Skeleton` document: `{"bones": [{"name", "parent",
/// "local": {"rotation", "translation", "scale"}}]}`.
fn skeleton_from_json(text: &str) -> Result<Skeleton, String> {
    let mut parser = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let root = parser.value(0)?;
    let bones = root
        .get("bones")
        .and_then(Json::arr)
        .ok_or("no `bones` array")?;
    let mut out = Vec::with_capacity(bones.len());
    for (i, b) in bones.iter().enumerate() {
        let bad = |what: &str| format!("bone {i}: {what}");
        let name = match b.get("name") {
            Some(Json::Str(s)) => s.clone(),
            _ => return Err(bad("no name")),
        };
        let parent = match b.get("parent") {
            Some(Json::Num(p)) if *p >= 0.0 && (*p as usize) < i => Some(*p as u32),
            Some(Json::Null) | None => None,
            _ => return Err(bad("bad parent")),
        };
        let local = b.get("local").ok_or_else(|| bad("no local"))?;
        let rows = local
            .get("rotation")
            .and_then(Json::arr)
            .ok_or_else(|| bad("no rotation"))?;
        if rows.len() != 3 {
            return Err(bad("rotation is not 3 rows"));
        }
        let mut rotation = [[0.0f32; 3]; 3];
        for (r, row) in rows.iter().enumerate() {
            let row = row
                .arr()
                .filter(|r| r.len() == 3)
                .ok_or_else(|| bad("bad rotation row"))?;
            for (c, v) in row.iter().enumerate() {
                rotation[r][c] = v.num().ok_or_else(|| bad("bad rotation value"))?;
            }
        }
        let t = local
            .get("translation")
            .and_then(Json::arr)
            .filter(|t| t.len() == 3)
            .ok_or_else(|| bad("bad translation"))?;
        let mut translation = [0.0f32; 3];
        for (k, v) in t.iter().enumerate() {
            translation[k] = v.num().ok_or_else(|| bad("bad translation value"))?;
        }
        let scale = local.get("scale").and_then(Json::num).unwrap_or(1.0);
        out.push(Bone {
            name,
            parent,
            local: BoneTransform {
                rotation,
                translation,
                scale,
            },
        });
    }
    Ok(Skeleton { bones: out })
}
