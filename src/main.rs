mod app;
mod config;
mod feed;
mod forms;
mod input;
mod tags;
mod theme;
mod ui;

use anyhow::{Context, Result, bail};
use ratatui_image::picker::Picker;

fn main() -> Result<()> {
    // Load before touching the terminal so config errors print normally.
    let paths = config::Paths::user()?;
    let config = config::Config::load(&paths.config)?;
    let themes = theme::load_themes(&paths.themes, &config.themes).context("loading themes")?;
    if theme::find(&themes, &config.theme).is_none() {
        let names: Vec<_> = themes.iter().map(|t| t.name.as_str()).collect();
        bail!(
            "{}: unknown theme '{}' (available: {})",
            paths.config.display(),
            config.theme,
            names.join(", ")
        );
    }
    let state = config::State::load(&paths.state);

    let mut terminal = ratatui::init();
    // Must run after entering the alternate screen and before reading events.
    let picker = if config.image_protocol == "halfblocks" {
        Picker::halfblocks()
    } else {
        Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
    };
    let result = app::run(&mut terminal, config, state, picker, themes, paths);
    ratatui::restore();
    result
}
