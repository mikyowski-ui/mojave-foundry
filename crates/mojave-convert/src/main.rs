//! The mojave-convert command line.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use foundry_render::Backend;
use mojave_convert::{
    ConvertError, Inputs, SliceSpec, critters, gallery, map, pipeline, probe, shots, sprites,
};

#[derive(Parser)]
#[command(
    name = "mojave-convert",
    about = "Converts your own Fallout: New Vegas install into a Fallout 2 format mod layer.",
    long_about = "Converts your own Fallout: New Vegas install into a Fallout 2 format mod layer.\n\n\
                  Both installs are required. Nothing is downloaded, and output must stay on\n\
                  the machine that owns the games."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct Common {
    /// The Fallout: New Vegas install directory (or its Data directory).
    #[arg(long)]
    fnv: PathBuf,
    /// The Fallout 2 install directory (holding master.dat).
    #[arg(long)]
    f2: PathBuf,
    /// Output directory. Keep it outside git.
    #[arg(long)]
    out: PathBuf,
    /// A built-in slice name, or a slice TOML file.
    #[arg(long, default_value = "goodsprings")]
    slice: String,
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
}

/// How the map stage treats Fallout 2's art lists.
#[derive(Args)]
struct Mode {
    /// Replace Fallout 2's wall and floor-tile lists instead of extending
    /// them (about 4,090 entries each, for demonstration maps). Stock maps
    /// draw the wrong walls and floors while such a layer is mounted.
    #[arg(long)]
    standalone: bool,
}

impl Common {
    fn inputs_in(&self, mode: &Mode) -> Result<Inputs, ConvertError> {
        let mut inputs = self.inputs()?;
        inputs.standalone = mode.standalone;
        Ok(inputs)
    }

    fn inputs(&self) -> Result<Inputs, ConvertError> {
        let slice = SliceSpec::resolve(&self.slice)?;
        let jobs = self
            .jobs
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(8)));
        let mut inputs = Inputs::new(&self.fnv, &self.f2, &self.out, slice, jobs)?;
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
    /// Walk a plugin and decode every record.
    Esm {
        plugin: PathBuf,
        /// Cell editor id substring to look at more closely.
        #[arg(long, default_value = "")]
        cell: String,
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
        Command::Critters { common, preview } => {
            let inputs = common.inputs().map_err(|e| e.to_string())?;
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
            println!("critters: {summary:?}");
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
        Command::All { common, mode } => {
            let inputs = common.inputs_in(&mode).map_err(|e| e.to_string())?;
            let summary = pipeline::all(&inputs).map_err(|e| e.to_string())?;
            println!("sprites: {:?}", summary.sprites);
            println!("critters: {:?}", summary.critters);
            println!("map: {:#?}", summary.map);
            println!("gallery: {:?}", summary.gallery);
            println!("report: {}", inputs.mir_dir().join("report.md").display());
            println!("layer: {}", inputs.mod_dir().display());
            Ok(())
        }
        Command::Estimate { common, mode } => {
            let started = std::time::Instant::now();
            let inputs = common.inputs_in(&mode).map_err(|e| e.to_string())?;
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
            };
            let summary = shots::shots(&options).map_err(|e| e.to_string())?;
            for shot in &summary.shots {
                println!(
                    "{:02} {:<4} hex {:>5} ({:>3} steps) arrived {:<3} camera {:<4} {} {}",
                    shot.viewpoint,
                    shot.view,
                    shot.hex,
                    shot.steps,
                    if shot.arrived { "yes" } else { "no" },
                    if shot.camera_followed { "on" } else { "held" },
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
        Command::Probe(Probe::Esm { plugin, cell }) => probe::esm(&plugin, &cell),
        Command::Probe(Probe::F2 { dir }) => probe::f2(&dir),
        Command::Probe(Probe::Walk {
            layer,
            map,
            hop,
            from,
        }) => probe::walk(&layer, &map, hop, &from),
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
