//! The terminal: raw mode, the alternate screen, the event loop.
pub mod app;
pub mod draw;

use crate::config::Config;
use crate::docker::Docker;
use crate::engine;
use app::{App, Foreground};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{stdout, Stdout};
use std::time::{Duration, Instant};

type Term = Terminal<CrosstermBackend<Stdout>>;

fn enter() -> Result<Term, String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture).map_err(|e| e.to_string())?;
    Terminal::new(CrosstermBackend::new(stdout())).map_err(|e| e.to_string())
}

pub fn leave() {
    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::Show
    );
}

/// Hand the terminal to a program (a shell, an editor), then take it back.
fn foreground(term: &mut Term, fg: Foreground) -> Result<String, String> {
    let Foreground::Command(args) = fg;
    leave();
    println!("\x1b[2m── {} ── (exit to return)\x1b[0m", args.join(" "));
    let status = std::process::Command::new(&args[0])
        .args(&args[1..])
        .status();
    *term = enter()?;
    term.clear().map_err(|e| e.to_string())?;
    match status {
        Ok(s) if s.success() => Ok(String::new()),
        Ok(s) => Err(format!("{} ended with {}", args[0], s.code().unwrap_or(-1))),
        Err(e) => Err(format!("{}: {e}", args[0])),
    }
}

pub fn run(cfg: Config) -> Result<(), String> {
    let docker = Docker::new()?;
    // The terminal comes back even after a panic.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave();
        hook(info);
    }));
    let (tx, rx) = engine::channel_pair();
    let poke = engine::refresher(docker.clone(), cfg.clone(), tx.clone());
    engine::events(docker.clone(), tx.clone(), poke.clone());
    let mut app = App::new(cfg, docker, tx, poke);
    let mut term = enter()?;
    let mut last_draw = Instant::now() - Duration::from_secs(1);
    let mut dirty = true;
    let result = loop {
        // Everything that arrived from the background.
        let mut n = 0;
        while let Ok(msg) = rx.try_recv() {
            app.handle(msg);
            dirty = true;
            n += 1;
            if n > 500 {
                break;
            }
        }
        if let Some(fg) = app.foreground.take() {
            match foreground(&mut term, fg) {
                Ok(_) => app.say("Back"),
                Err(e) => app.warn(e),
            }
            let _ = app.poke.send(());
            dirty = true;
        }
        // Once a second at least: the clock, spinners, the flash message.
        if dirty || last_draw.elapsed() >= Duration::from_secs(1) {
            if let Err(e) = term.draw(|f| draw::draw(f, &mut app)) {
                break Err(e.to_string());
            }
            last_draw = Instant::now();
            dirty = false;
        }
        if app.quit {
            break Ok(());
        }
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                    app.key(k);
                    dirty = true;
                }
                Ok(Event::Mouse(m)) => {
                    app.mouse(m);
                    dirty = true;
                }
                Ok(Event::Resize(..)) => dirty = true,
                _ => {}
            },
            Ok(false) => {}
            Err(e) => break Err(e.to_string()),
        }
    };
    leave();
    result
}
