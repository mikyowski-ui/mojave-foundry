//! The mojave-convert command line.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use foundry_render::Backend;
use mojave_convert::{
    ConvertError, GameVariant, Inputs, Install, critters, gallery, map, pipeline, probe, sequence,
    shots, sprites,
};

#[derive(Parser)]
#[command(
    name = "mojave-convert",
    about = "Converts your own Fallout 3 or Fallout: New Vegas install into a Fallout 2 format mod layer.",
    long_about = "Converts your own Fallout 3 or Fallout: New Vegas install into a Fallout 2 format\n\
                  mod layer.\n\n\
                  The source install (--game-dir, or --fo3 / --fnv) and a Fallout 2 install (--f2)\n\
                  are both required. The game is read from the install's master plugin unless\n\
                  --game names it. Nothing is downloaded, and output must stay on the machine\n\
                  that owns the games."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("source").required(true).args(["game_dir", "fnv", "fo3"])))]
struct Common {
    /// The source install directory (or its Data directory): Fallout 3 or
    /// Fallout: New Vegas, told apart by the master plugin it holds.
    #[arg(long, value_name = "DIR")]
    game_dir: Option<PathBuf>,
    /// The source game, when the install should not decide it: fo3 or fnv.
    /// A slice, an install or an alias flag for the other game is an error.
    #[arg(long, value_name = "GAME")]
    game: Option<String>,
    /// A Fallout: New Vegas install: --game-dir with --game fnv.
    #[arg(long, value_name = "DIR")]
    fnv: Option<PathBuf>,
    /// A Fallout 3 install: --game-dir with --game fo3.
    #[arg(long, value_name = "DIR")]
    fo3: Option<PathBuf>,
    /// The Fallout 2 install directory (holding master.dat).
    #[arg(long)]
    f2: PathBuf,
    /// Output directory. Keep it outside git.
    #[arg(long)]
    out: PathBuf,
    /// A built-in slice name (goodsprings for New Vegas, springvale for
    /// Fallout 3), or a slice TOML file. Default: the game's built-in one.
    #[arg(long)]
    slice: Option<String>,
    /// Worker threads. Output never depends on this.
    #[arg(long)]
    jobs: Option<usize>,
    /// Calibration TOML (scale, supersampling, lighting). Defaults apply
    /// when absent.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Render path: cpu (same bytes on every machine), gpu, or auto.
    #[arg(long, default_value = "cpu")]
    backend: String,
    /// Draw a one-pixel dark inner edge on sprites.
    #[arg(long)]
    outline: bool,
    /// A rebalance TOML laid over the game's defaults. Default:
    /// rebalance/<game>.toml beside the slice file or above it, else the
    /// built-in tables.
    #[arg(long, value_name = "FILE")]
    rebalance: Option<PathBuf>,
}

/// The critter stage's options; each wins over the slice's key.
#[derive(Args, Default)]
struct CritterArgs {
    /// How many animations a sprite set carries: minimal (default; the
    /// first wave's stand, walk and stills), combat or full.
    #[arg(long, value_name = "LEVEL")]
    animations: Option<String>,
    /// Falls and corpses: ragdoll or game.
    #[arg(long, value_name = "SOURCE")]
    deaths: Option<String>,
    /// Walk travel against the grid: exact, matched or whole_hexes.
    #[arg(long, value_name = "RULE")]
    stride: Option<String>,
    /// How figures are drawn: default or readable.
    #[arg(long, value_name = "LOOK")]
    figure_look: Option<String>,
    /// Render only the actors with this editor id (repeatable).
    #[arg(long, value_name = "EDITOR_ID")]
    only: Vec<String>,
}

impl CritterArgs {
    fn apply(&self, inputs: &mut Inputs) -> Result<(), ConvertError> {
        let bad = |e: String| ConvertError::stage("arguments", format!("--{e}"));
        let settings = &mut inputs.critter;
        if let Some(level) = &self.animations {
            settings.level = Some(
                level
                    .parse::<critters::AnimationLevel>()
                    .map_err(|e| bad(format!("animations {e}")))?,
            );
        }
        if let Some(deaths) = &self.deaths {
            settings.deaths = Some(mojave_convert::config::parse_deaths(deaths).map_err(bad)?);
        }
        if let Some(stride) = &self.stride {
            settings.stride = Some(mojave_convert::config::parse_stride(stride).map_err(bad)?);
        }
        if let Some(look) = &self.figure_look {
            settings.look = Some(mojave_convert::config::parse_look(look).map_err(bad)?);
        }
        settings.only = self.only.clone();
        Ok(())
    }
}

/// How the map stage treats Fallout 2's art lists.
#[derive(Args)]
struct Mode {
    /// Replace Fallout 2's wall and floor-tile lists instead of extending
    /// them (about 4,090 entries each, for demonstration maps). Stock maps
    /// draw the wrong walls and floors while such a layer is mounted.
    #[arg(long)]
    standalone: bool,
    /// The declutter and readability pass: off or readable. Default: the
    /// slice's `[declutter] profile`, else off.
    #[arg(long, value_name = "PROFILE")]
    declutter: Option<String>,
    /// Items, filled containers, locked doors, armed and sided people: on
    /// or off. Default: the slice's `loot`, else off.
    #[arg(long, value_name = "ON|OFF")]
    loot: Option<String>,
    /// On an exterior slice: build every interior its doors lead to (depth
    /// 1) into the same layer, the doors linked both ways.
    #[arg(long)]
    with_interiors: bool,
}

impl Common {
    fn inputs_in(&self, mode: &Mode) -> Result<Inputs, ConvertError> {
        let mut inputs = self.inputs()?;
        inputs.standalone = mode.standalone;
        inputs.with_interiors = mode.with_interiors;
        if let Some(name) = &mode.declutter {
            if !mojave_convert::declutter::PROFILES.contains(&name.as_str()) {
                return Err(ConvertError::stage(
                    "arguments",
                    format!(
                        "--declutter {name}: expected one of {}",
                        mojave_convert::declutter::PROFILES.join(", ")
                    ),
                ));
            }
            inputs.declutter = Some(name.clone());
        }
        inputs.loot = match mode.loot.as_deref() {
            None => None,
            Some("on") => Some(true),
            Some("off") => Some(false),
            Some(other) => {
                return Err(ConvertError::stage(
                    "arguments",
                    format!("--loot {other}: expected on or off"),
                ));
            }
        };
        Ok(inputs)
    }

    /// The source install, from whichever of --game-dir, --fnv and --fo3
    /// was given, and --game.
    fn install(&self) -> Result<Install, ConvertError> {
        let asked = match &self.game {
            Some(text) => Some(text.parse::<GameVariant>().map_err(|_| {
                ConvertError::stage("arguments", format!("--game {text}: expected fo3 or fnv"))
            })?),
            None => None,
        };
        let install = match (&self.game_dir, &self.fnv, &self.fo3) {
            (Some(dir), None, None) => Install::any(dir, asked),
            (None, Some(dir), None) => Install::new_vegas(dir),
            (None, None, Some(dir)) => Install::fallout3(dir),
            _ => {
                return Err(ConvertError::stage(
                    "arguments",
                    "give exactly one of --game-dir, --fnv and --fo3",
                ));
            }
        };
        if let (Some(asked), Some(fixed)) = (asked, install.game)
            && asked != fixed
        {
            return Err(ConvertError::stage(
                "arguments",
                format!(
                    "{} is a {} install, but --game asks for {}",
                    install.flag,
                    fixed.display_name(),
                    asked.display_name()
                ),
            ));
        }
        Ok(install)
    }

    fn inputs(&self) -> Result<Inputs, ConvertError> {
        let jobs = self
            .jobs
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(8)));
        let install = self.install()?;
        let mut inputs =
            Inputs::from_args(&install, self.slice.as_deref(), &self.f2, &self.out, jobs)?;
        if let Some(config) = &self.config {
            inputs = inputs.with_calibration_file(config)?;
        }
        inputs.backend = match self.backend.as_str() {
            "cpu" => Backend::Cpu,
            "gpu" => Backend::Gpu,
            "auto" => Backend::Auto,
            other => {
                return Err(ConvertError::stage(
                    "arguments",
                    format!("--backend {other}: expected cpu, gpu or auto"),
                ));
            }
        };
        inputs.outline = self.outline;
        inputs.rebalance = self.rebalance.clone();
        Ok(inputs)
    }
}

#[derive(Subcommand)]
enum Command {
    /// Extract the slice to the intermediate directory and write report.md.
    Extract(Common),
    /// Render every prop mesh of the slice to a palettized FRM.
    Sprites(Common),
    /// Render every human appearance of the slice as a critter sprite set.
    Critters {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        animation: CritterArgs,
        /// Instead, write a large picture of this actor (by editor id) to
        /// gallery/actor-<id>.png, for tuning.
        #[arg(long, value_name = "EDITOR_ID")]
        preview: Option<String>,
    },
    /// Build the slice's Fallout 2 map and the mod layer that carries it.
    Map {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        mode: Mode,
    },
    /// Run every stage: extract, sprites, critters, map, gallery.
    All {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        mode: Mode,
        #[command(flatten)]
        animation: CritterArgs,
    },
    /// After extract: the slicer's pre-check of the map window against the
    /// art budget of the chosen mode, in seconds, rendering nothing.
    Estimate {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        mode: Mode,
    },
    /// Screenshots of a converted map, taken by the engine itself, headless.
    Shots(ShotsArgs),
    /// A frame sequence and animated GIF of one walk, taken by the engine.
    Walkthrough(sequence::WalkthroughArgs),
    /// Frames and a GIF of a recorded run (an autoplay log), replayed by
    /// the engine a prefix at a time.
    ReplayFrames(sequence::ReplayFramesArgs),
    /// A contact page of PNG files, each under its label, scaled to fit.
    Contact {
        /// The page to write.
        #[arg(long)]
        out: PathBuf,
        /// A title line.
        #[arg(long)]
        title: Option<String>,
        /// Pictures to a row.
        #[arg(long, default_value_t = 4)]
        columns: u32,
        /// Largest picture area per cell, `WIDTHxHEIGHT` pixels.
        #[arg(long, default_value = "480x270")]
        cell: String,
        /// `LABEL=FILE.png`, or a file alone (labelled by its name).
        pictures: Vec<String>,
    },
    /// A page pairing two runs' map previews (gallery/map-*.png): the
    /// declutter pass before and after.
    BeforeAfter {
        /// The --out directory of the run without the pass.
        #[arg(long)]
        before: PathBuf,
        /// The --out directory of the run with it.
        #[arg(long)]
        after: PathBuf,
        /// The page to write.
        #[arg(long)]
        out: PathBuf,
    },
    /// Write contact sheets of the rendered sprites.
    Gallery {
        #[command(flatten)]
        common: Common,
        /// Keep only sprites whose model path contains this text.
        #[arg(long, default_value = "")]
        filter: String,
    },
    /// Read-only checks of the parsers against real files. Prints tallies only.
    #[command(subcommand)]
    Probe(Probe),
}

#[derive(Args)]
struct ShotsArgs {
    /// A vault-platform executable.
    #[arg(long)]
    engine: PathBuf,
    /// The Fallout 2 install directory (holding master.dat and critter.dat).
    #[arg(long)]
    f2: PathBuf,
    /// The --out directory of a converted slice (holding mods/mojave).
    /// Shots go to <out>/shots/.
    #[arg(long)]
    out: PathBuf,
    /// A hex to walk to and photograph (repeatable). An unreachable hex is
    /// replaced by the reachable hex nearest it.
    #[arg(long, value_name = "HEX")]
    at: Vec<i32>,
    /// Pick up to N viewpoints that show the map off: the entry, then the
    /// reachable hexes nearest its largest landmarks and buildings (on an
    /// interior demo, nearest its people first).
    #[arg(long, value_name = "N", default_value_t = 0)]
    auto: usize,
    /// The map's file name under maps/ (default: the map stage's exterior
    /// map).
    #[arg(long)]
    map: Option<String>,
    /// Steps per walk command; the engine's route search gives up on long
    /// ones.
    #[arg(long, default_value_t = 15)]
    hop: usize,
    /// Fewest hex steps between two automatic viewpoints (default 30, or
    /// 5 on an interior demo).
    #[arg(long)]
    spread: Option<i32>,
    /// The wide view's zoom (default 0.5, or 0.75 on an interior demo).
    #[arg(long)]
    wide_zoom: Option<f32>,
    /// Engine processes at once.
    #[arg(long, default_value_t = 2)]
    jobs: usize,
    /// Seconds an engine run may take before it is stopped.
    #[arg(long, default_value_t = 600)]
    timeout: u64,
    #[command(flatten)]
    views: shots::ViewArgs,
}

#[derive(Subcommand)]
enum Probe {
    /// Open an archive, verify name hashes, read every entry.
    Bsa {
        archive: PathBuf,
        #[arg(long, default_value_t = 1)]
        step: usize,
        /// Also decode every texture.
        #[arg(long)]
        dds: bool,
    },
    /// Parse every mesh in an archive and round-trip a sample through glb.
    Nif {
        archive: PathBuf,
        #[arg(long, default_value_t = 1)]
        step: usize,
        #[arg(long, default_value = "")]
        prefix: String,
    },
    /// Walk a plugin (either game), show its game, and decode every record.
    Esm {
        plugin: PathBuf,
        /// Cell editor id substring to look at more closely.
        #[arg(long, default_value = "")]
        cell: String,
        /// Cell editor id substring: break the matching cells' placed
        /// references down by the record type of their base.
        #[arg(long, default_value = "")]
        cells: String,
    },
    /// Open a Fallout 2 install and read its palette.
    F2 { dir: PathBuf },
    /// Read a map of a written mod layer and report what a walker can
    /// reach from its entry hex, with waypoints to every exit.
    Walk {
        /// The mod layer directory (the one holding vault-mod.toml).
        layer: PathBuf,
        /// The map's file name under maps/.
        map: String,
        /// The elevation to walk (0 to 2); on one that is not the map's
        /// entry elevation the walk starts at its stairs' first arrival.
        #[arg(long, default_value_t = 0)]
        elevation: usize,
        /// Steps between printed waypoints.
        #[arg(long, default_value_t = 20)]
        hop: usize,
        /// An arrival hex to test (repeatable): what it reaches without
        /// stepping on an exit grid.
        #[arg(long)]
        from: Vec<i32>,
    },
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Extract(common) => {
            let inputs = common.inputs().map_err(|e| e.to_string())?;
            let summary = pipeline::extract(&inputs).map_err(|e| e.to_string())?;
            println!("extract: {summary:#?}");
            println!("report: {}", inputs.mir_dir().join("report.md").display());
            Ok(())
        }
        Command::Sprites(common) => {
            let inputs = common.inputs().map_err(|e| e.to_string())?;
            let summary = sprites::sprites(&inputs).map_err(|e| e.to_string())?;
            println!("sprites: {summary:?}");
            println!("output: {}", inputs.sprites_dir().display());
            Ok(())
        }
        Command::Critters {
            common,
            animation,
            preview,
        } => {
            let mut inputs = common.inputs().map_err(|e| e.to_string())?;
            animation.apply(&mut inputs).map_err(|e| e.to_string())?;
            if let Some(id) = preview {
                let (rgba, width, height) =
                    critters::preview(&inputs, &id, 5.0, &[2, 1, 5]).map_err(|e| e.to_string())?;
                let dir = inputs.gallery_dir();
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let path = dir.join(format!("actor-{}.png", id.to_ascii_lowercase()));
                let png =
                    foundry_gallery::encode_png(&rgba, width, height).map_err(|e| e.to_string())?;
                std::fs::write(&path, png).map_err(|e| e.to_string())?;
                println!("preview: {}", path.display());
                return Ok(());
            }
            let summary = critters::critters(&inputs).map_err(|e| e.to_string())?;
            for line in &summary.lines {
                println!("  {line}");
            }
            println!(
                "critters: {} actors, {} sets ({} rendered, {} reused, {} failed)",
                summary.actors, summary.sets, summary.rendered, summary.reused, summary.failed
            );
            println!("output: {}", inputs.critters_dir().display());
            Ok(())
        }
        Command::Map { common, mode } => {
            let inputs = common.inputs_in(&mode).map_err(|e| e.to_string())?;
            let summary = map::map(&inputs).map_err(|e| e.to_string())?;
            println!("map: {summary:#?}");
            println!("layer: {}", inputs.mod_dir().display());
            println!("report: {}", inputs.out.join("map-report.md").display());
            Ok(())
        }
        Command::All {
            common,
            mode,
            animation,
        } => {
            let mut inputs = common.inputs_in(&mode).map_err(|e| e.to_string())?;
            animation.apply(&mut inputs).map_err(|e| e.to_string())?;
            let summary = pipeline::all(&inputs).map_err(|e| e.to_string())?;
            println!("sprites: {:?}", summary.sprites);
            for line in &summary.critters.lines {
                println!("  {line}");
            }
            let c = &summary.critters;
            println!(
                "critters: {} actors, {} sets ({} rendered, {} reused, {} failed)",
                c.actors, c.sets, c.rendered, c.reused, c.failed
            );
            println!("map: {:#?}", summary.map);
            println!("gallery: {:?}", summary.gallery);
            println!("report: {}", inputs.mir_dir().join("report.md").display());
            println!("layer: {}", inputs.mod_dir().display());
            Ok(())
        }
        Command::Estimate { common, mode } => {
            let started = std::time::Instant::now();
            let inputs = common.inputs_in(&mode).map_err(|e| e.to_string())?;
            if let Some(layer) = inputs.layer_spec() {
                inputs.check_mode().map_err(|e| e.to_string())?;
                let text = mojave_convert::layer::estimate_layer(&inputs, &layer)
                    .map_err(|e| e.to_string())?;
                println!("estimate for the layer {}:\n{text}", layer.name);
                println!("time: {:.1} s", started.elapsed().as_secs_f64());
                return Ok(());
            }
            let summary = map::estimate(&inputs).map_err(|e| e.to_string())?;
            println!("{summary}");
            println!("\nDetail:\n{}", summary.report);
            println!("time: {:.1} s", started.elapsed().as_secs_f64());
            Ok(())
        }
        Command::Shots(args) => {
            let options = shots::ShotsOptions {
                engine: args.engine,
                f2: args.f2,
                out: args.out,
                map: args.map,
                at: args.at,
                auto: args.auto,
                hop: args.hop.max(1),
                spread: args.spread,
                wide_zoom: args.wide_zoom,
                jobs: args.jobs,
                timeout: std::time::Duration::from_secs(args.timeout),
                framing: args.views.framing()?,
                dest: args.views.dest,
            };
            let summary = shots::shots(&options).map_err(|e| e.to_string())?;
            for shot in &summary.shots {
                println!(
                    "{:02} {:<7} hex {:>5} ({:>3} steps) arrived {:<3} camera {:<21} {} {}",
                    shot.viewpoint,
                    shot.view,
                    shot.hex,
                    shot.steps,
                    if shot.arrived { "yes" } else { "no" },
                    shot.camera,
                    if shot.file.is_empty() {
                        "-"
                    } else {
                        &shot.file
                    },
                    shot.error.as_deref().unwrap_or("")
                );
            }
            println!(
                "shots: {} viewpoints of {}; index {}",
                summary.viewpoints,
                summary.map,
                summary.dir.join("index.json").display()
            );
            Ok(())
        }
        Command::Walkthrough(args) => sequence::walkthrough_cli(&args),
        Command::ReplayFrames(args) => sequence::replay_frames_cli(&args),
        Command::Contact {
            out,
            title,
            columns,
            cell,
            pictures,
        } => {
            let max_cell = cell
                .split_once('x')
                .and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?]))
                .ok_or_else(|| format!("--cell {cell}: expected WIDTHxHEIGHT"))?;
            let entries: Vec<(String, PathBuf)> = pictures
                .iter()
                .map(|p| match p.split_once('=') {
                    Some((label, file)) => (label.to_owned(), PathBuf::from(file)),
                    None => {
                        let path = PathBuf::from(p);
                        let label = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        (label, path)
                    }
                })
                .collect();
            shots::contact_page(&entries, title.as_deref(), columns, max_cell, &out)
                .map_err(|e| e.to_string())?;
            println!("contact: {}", out.display());
            Ok(())
        }
        Command::BeforeAfter { before, after, out } => {
            gallery::before_after_page(&before, &after, ["before", "after"], &out)
                .map_err(|e| e.to_string())?;
            println!("before/after: {}", out.display());
            Ok(())
        }
        Command::Gallery { common, filter } => {
            let inputs = common.inputs().map_err(|e| e.to_string())?;
            let summary = gallery::gallery(&inputs, &filter).map_err(|e| e.to_string())?;
            println!("gallery: {summary:?}");
            println!("output: {}", inputs.gallery_dir().display());
            Ok(())
        }
        Command::Probe(Probe::Bsa { archive, step, dds }) => probe::bsa(&archive, step, dds),
        Command::Probe(Probe::Nif {
            archive,
            step,
            prefix,
        }) => probe::nif(&archive, step, &prefix),
        Command::Probe(Probe::Esm {
            plugin,
            cell,
            cells,
        }) => probe::esm(&plugin, &cell, &cells),
        Command::Probe(Probe::F2 { dir }) => probe::f2(&dir),
        Command::Probe(Probe::Walk {
            layer,
            map,
            elevation,
            hop,
            from,
        }) => probe::walk(&layer, &map, elevation, hop, &from),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
