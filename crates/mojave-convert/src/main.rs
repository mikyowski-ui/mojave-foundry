//! The mojave-convert command line.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use foundry_render::Backend;
use mojave_convert::{
    ConvertError, Inputs, SliceSpec, critters, gallery, map, pipeline, probe, sprites,
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

impl Common {
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
    Map(Common),
    /// Run every stage: extract, sprites, critters, map, gallery.
    All(Common),
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
        Command::Map(common) => {
            let inputs = common.inputs().map_err(|e| e.to_string())?;
            let summary = map::map(&inputs).map_err(|e| e.to_string())?;
            println!("map: {summary:#?}");
            println!("layer: {}", inputs.mod_dir().display());
            println!("report: {}", inputs.out.join("map-report.md").display());
            Ok(())
        }
        Command::All(common) => {
            let inputs = common.inputs().map_err(|e| e.to_string())?;
            let summary = pipeline::all(&inputs).map_err(|e| e.to_string())?;
            println!("sprites: {:?}", summary.sprites);
            println!("critters: {:?}", summary.critters);
            println!("map: {:#?}", summary.map);
            println!("gallery: {:?}", summary.gallery);
            println!("report: {}", inputs.mir_dir().join("report.md").display());
            println!("layer: {}", inputs.mod_dir().display());
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
