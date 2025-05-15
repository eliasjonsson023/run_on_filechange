// run_on_filechange.rs
// Usage example:
//   ./run_on_filechange "cargo run --release" ./src ./tests

use chrono::Local;
use nix::libc;
use nix::sys::signal::{Signal::SIGTERM, kill};
use nix::unistd::Pid;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::os::unix::process::CommandExt; // for .pre_exec
use std::{
  env,
  path::PathBuf,
  process::{Child, Command, Stdio},
  sync::mpsc::channel,
  time::{Duration, Instant},
};

fn log(msg: &str) {
  let now = Local::now().format("%Y-%m-%d %H:%M:%S");
  println!("{now}: {msg}");
}

fn main() -> notify::Result<()> {
  // ----------- Parse CLI --------------------------------------------------
  let mut args = env::args().skip(1); // skip program name
  let cmd_string = args.next().unwrap_or_else(|| {
    eprintln!("Usage:\n  run_on_filechange \"<command>\" <dir1> [dir2] …");
    std::process::exit(1);
  });
  let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
  if paths.is_empty() {
    eprintln!("Error: at least one directory must be given.");
    std::process::exit(1);
  }

  // ----------- Validate paths ---------------------------------------------
  for p in &paths {
    if !p.is_dir() {
      eprintln!("Error: {:?} is not a directory.", p);
      std::process::exit(1);
    }
  }

  // ----------- File‑watcher setup -----------------------------------------
  let (tx, rx) = channel();
  let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
  for p in &paths {
    watcher.watch(p, RecursiveMode::Recursive)?;
    log(&format!("Watching {:?}", p));
  }

  // ----------- Event loop --------------------------------------------------
  let mut last_event: Option<Instant> = None;
  let debounce = Duration::from_millis(8_000);
  let mut child: Option<Child> = None;

  while let Ok(event) = rx.recv() {
    match event {
      Ok(Event { kind, .. }) => {
        if !matches!(
          kind,
          EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
          continue; // ignore other kinds
        }

        // debounce
        if let Some(t) = last_event {
          if t.elapsed() < debounce {
            continue;
          }
        }
        last_event = Some(Instant::now());
        log("File change detected");

        // Kill previous run if still alive
        if let Some(mut c) = child.take() {
          let pgid = -(c.id() as i32); // negative ⇒ process‑group id
          kill(Pid::from_raw(pgid), SIGTERM).ok(); // politely ask entire group
          let _ = c.wait(); // reap the leader
          std::thread::sleep(Duration::from_millis(700)); // TIME_WAIT drain
        }

        // Spawn new run
        log(&format!("Executing: {cmd_string}"));
        let tmp_child = unsafe {
          // <- acknowledge the unsafety
          Command::new("/bin/sh")
            .arg("-c")
            .arg(&cmd_string)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .pre_exec(|| {
              // SAFETY: setpgid is async‑signal‑safe and we do nothing else here
              unsafe { libc::setpgid(0, 0) };
              Ok(())
            })
            .spawn()
        }?; //  <- keep the Result from spawn()
        child = Some(tmp_child);
      }
      Err(e) => log(&format!("Watcher error: {e:?}")),
    }
  }
  Ok(())
}
