use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::{Result, bail};
use ratatui::{
    backend::CrosstermBackend,
    widgets::{Block, Borders, Paragraph},
    layout::{Layout, Constraint, Direction, Alignment},
    style::{Style, Color, Modifier},
    Terminal,
};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

struct MenuItem {
    label: String,
    description: String,
    action: Box<dyn Fn() -> Result<()>>,
}

struct AppState {
    menu_items: Vec<MenuItem>,
    selected: usize,
    scroll_offset: usize,
    keyboard_section_expanded: bool,
    locale_section_expanded: bool,
    current_layout: String,
    current_locale: String,
}

impl AppState {
    fn new() -> Self {
        Self {
            menu_items: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            keyboard_section_expanded: true,
            locale_section_expanded: true,
            current_layout: String::new(),
            current_locale: String::new(),
        }
    }

    fn refresh_status(&mut self) {
        self.current_layout = get_current_keyboard_layout();
        self.current_locale = get_current_locale();
    }

    fn build_menu(&mut self) {
        self.menu_items.clear();

        let available_locales = get_available_locales();
        let available_keyboard_layouts = get_available_keyboard_layouts();

        // Add keyboard layout section
        if !available_keyboard_layouts.is_empty() {
            let expand_symbol = if self.keyboard_section_expanded { "▼" } else { "▶" };
            self.menu_items.push(MenuItem {
                label: format!("{} Keyboard Layouts", expand_symbol),
                description: format!("Current: {}", self.current_layout),
                action: Box::new(|| Ok(())),
            });

            if self.keyboard_section_expanded {
                for (layout_code, display_name) in available_keyboard_layouts {
                    let layout_code_clone = layout_code.clone();
                    let is_current = layout_code == self.current_layout;
                    let prefix = if is_current { "● " } else { "  " };
                    self.menu_items.push(MenuItem {
                        label: format!("{}{}", prefix, display_name),
                        description: format!("Layout: {}", layout_code),
                        action: Box::new(move || switch_to_keyboard_layout(&layout_code_clone)),
                    });
                }
            }
        }

        // Add locale section
        let expand_symbol = if self.locale_section_expanded { "▼" } else { "▶" };
        self.menu_items.push(MenuItem {
            label: format!("{} System Locales", expand_symbol),
            description: format!("Current: {}", self.current_locale),
            action: Box::new(|| Ok(())),
        });

        if self.locale_section_expanded {
            for (locale_code, display_name) in available_locales {
                let locale_code_clone = locale_code.clone();
                let is_current = locale_code == self.current_locale;
                let prefix = if is_current { "● " } else { "  " };
                self.menu_items.push(MenuItem {
                    label: format!("{}{}", prefix, display_name),
                    description: locale_code.clone(),
                    action: Box::new(move || set_locale(&locale_code_clone)),
                });
            }
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        } else {
            self.selected = self.menu_items.len().saturating_sub(1);
        }
        self.adjust_scroll();
    }

    fn move_down(&mut self) {
        if self.selected < self.menu_items.len().saturating_sub(1) {
            self.selected += 1;
        } else {
            self.selected = 0;
        }
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        // Calculate visible area (items that can fit in the menu area)
        let visible_items = 10; // Conservative estimate, will be adjusted in render

        // Keep selected item visible
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_items {
            self.scroll_offset = self.selected.saturating_sub(visible_items - 1);
        }

        // Ensure we don't scroll past the end
        let max_scroll = self.menu_items.len().saturating_sub(visible_items);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    fn adjust_scroll_for_height(&mut self, visible_items: usize) {
        if visible_items == 0 {
            return;
        }

        // Keep selected item visible
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_items {
            self.scroll_offset = self.selected.saturating_sub(visible_items - 1);
        }

        // Ensure we don't scroll past the end
        let max_scroll = self.menu_items.len().saturating_sub(visible_items);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    fn toggle_section(&mut self) {
        if self.menu_items.is_empty() {
            return;
        }

        let item = &self.menu_items[self.selected];

        if item.label.contains("Keyboard Layouts") {
            self.keyboard_section_expanded = !self.keyboard_section_expanded;
            self.build_menu();
            // Keep selection on the keyboard header
            for (i, item) in self.menu_items.iter().enumerate() {
                if item.label.contains("Keyboard Layouts") {
                    self.selected = i;
                    break;
                }
            }
        } else if item.label.contains("System Locales") {
            self.locale_section_expanded = !self.locale_section_expanded;
            self.build_menu();
            // Keep selection on the locale header
            for (i, item) in self.menu_items.iter().enumerate() {
                if item.label.contains("System Locales") {
                    self.selected = i;
                    break;
                }
            }
        }
        self.adjust_scroll();
    }

    fn execute_selected(&mut self) -> Result<bool> {
        if self.menu_items.is_empty() {
            return Ok(false);
        }

        let item = &self.menu_items[self.selected];

        // Check if it's a header (expandable section)
        if item.label.contains("Keyboard Layouts") || item.label.contains("System Locales") {
            self.toggle_section();
            return Ok(false);
        }

        // Execute regular action
        let result = (item.action)();

        // Refresh status after any action
        self.refresh_status();
        self.build_menu();

        result.map(|_| false)
    }
}

fn notify(msg: &str) {
    let _ = Command::new("notify-send")
        .arg("Levocale")
        .arg(msg)
        .arg("-t")
        .arg("2000")
        .spawn();
}

fn get_current_keyboard_layout() -> String {
    // Try hyprctl first
    if let Ok(output) = Command::new("hyprctl").args(["devices"]).output() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        // Look for keyboard section and active layout
        for line in output_str.lines() {
            if line.contains("active keymap:") {
                if let Some(layout) = line.split("active keymap:").nth(1) {
                    return layout.trim().to_string();
                }
            }
        }
    }

    // Fallback to setxkbmap
    if let Ok(output) = Command::new("setxkbmap").args(["-query"]).output() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            if line.starts_with("layout:") {
                if let Some(layout) = line.split(':').nth(1) {
                    return layout.trim().to_string();
                }
            }
        }
    }

    "unknown".to_string()
}

fn get_current_locale() -> String {
    // Try reading from locale command first (more reliable)
    if let Ok(output) = Command::new("locale").output() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            if line.starts_with("LANG=") {
                if let Some(locale) = line.split('=').nth(1) {
                    return locale.trim_matches('"').to_string();
                }
            }
        }
    }

    // Fallback to localectl
    if let Ok(output) = Command::new("localectl").args(["status"]).output() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            if line.trim().starts_with("LANG=") {
                if let Some(locale) = line.split('=').nth(1) {
                    return locale.trim().to_string();
                }
            }
        }
    }

    // Last resort: check environment variable
    std::env::var("LANG").unwrap_or_else(|_| "unknown".to_string())
}

fn locale_to_keyboard_layout(locale_code: &str) -> Option<String> {
    // Map locale codes to keyboard layout codes
    let layout_code = if let Some(lang_country) = locale_code.split('.').next() {
        if let Some((lang, country)) = lang_country.split_once('_') {
            match lang {
                "en" => "us".to_string(),  // English uses US layout
                "da" => "dk".to_string(),  // Danish uses DK layout
                "de" => "de".to_string(),  // German
                "es" => "es".to_string(),  // Spanish
                "fr" => "fr".to_string(),  // French
                "zh" => "cn".to_string(),  // Chinese
                "ja" => "jp".to_string(),  // Japanese
                "ko" => "kr".to_string(),  // Korean
                "ru" => "ru".to_string(),  // Russian
                "it" => "it".to_string(),  // Italian
                "pt" => match country {
                    "BR" => "br".to_string(),  // Brazilian Portuguese
                    _ => "pt".to_string(),     // Portuguese
                },
                "nl" => "nl".to_string(),  // Dutch
                "sv" => "se".to_string(),  // Swedish
                "no" => "no".to_string(),  // Norwegian
                "fi" => "fi".to_string(),  // Finnish
                "pl" => "pl".to_string(),  // Polish
                "cs" => "cz".to_string(),  // Czech
                "hu" => "hu".to_string(),  // Hungarian
                "tr" => "tr".to_string(),  // Turkish
                "ar" => "ara".to_string(), // Arabic
                "hi" => "in".to_string(),  // Hindi (India layout)
                "th" => "th".to_string(),  // Thai
                "vi" => "vn".to_string(),  // Vietnamese
                _ => return None,  // Unsupported language
            }
        } else {
            // Handle cases without country code
            match lang_country {
                "C" => return None,  // C locale doesn't have a keyboard layout
                _ => return None,
            }
        }
    } else {
        return None;
    };

    Some(layout_code)
}

fn get_available_keyboard_layouts() -> Vec<(String, String)> {
    let mut layouts = Vec::new();
    let available_locales = get_available_locales();

    for (locale_code, display_name) in available_locales {
        if let Some(layout_code) = locale_to_keyboard_layout(&locale_code) {
            layouts.push((layout_code, display_name));
        }
    }

    // Remove duplicates (e.g., if multiple English locales map to "us")
    layouts.sort_by(|a, b| a.0.cmp(&b.0));
    layouts.dedup_by(|a, b| a.0 == b.0);

    layouts
}

fn switch_to_keyboard_layout(layout_code: &str) -> Result<()> {
    let result = Command::new("hyprctl")
        .args(["keyword", "input:kb_layout", layout_code])
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                notify(&format!("Keyboard layout set to: {}", layout_code));
                Ok(())
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                notify(&format!("Failed to set keyboard layout: {}", error.trim()));
                bail!("Failed to set keyboard layout: {}", error.trim())
            }
        }
        Err(e) => {
            notify(&format!("Failed to execute hyprctl: {}", e));
            bail!("Failed to execute hyprctl: {}", e)
        }
    }
}

fn get_available_locales() -> Vec<(String, String)> {
    let mut locales = Vec::new();

    if let Ok(output) = Command::new("localectl").args(["list-locales"]).output() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            let locale_code = line.trim().to_string();
            if !locale_code.is_empty() {
                // Create a display name from the locale code
                let display_name = locale_code_to_display_name(&locale_code);
                locales.push((locale_code, display_name));
            }
        }
    }

    // If localectl fails, return a minimal fallback
    if locales.is_empty() {
        locales.push(("en_US.UTF-8".to_string(), "English (US)".to_string()));
        locales.push(("C.UTF-8".to_string(), "C (POSIX)".to_string()));
    }

    locales
}

fn locale_code_to_display_name(locale_code: &str) -> String {
    // Convert locale codes to human-readable names
    match locale_code {
        "C" | "C.UTF-8" => "C (POSIX)".to_string(),
        code if code.starts_with("en_US") => "English (US)".to_string(),
        code if code.starts_with("en_GB") => "English (UK)".to_string(),
        code if code.starts_with("da_DK") => "Danish (Denmark)".to_string(),
        code if code.starts_with("de_DE") => "German (Germany)".to_string(),
        code if code.starts_with("es_US") => "Spanish (US)".to_string(),
        code if code.starts_with("es_ES") => "Spanish (Spain)".to_string(),
        code if code.starts_with("fr_FR") => "French (France)".to_string(),
        code if code.starts_with("zh_CN") => "Chinese (Simplified)".to_string(),
        code if code.starts_with("zh_TW") => "Chinese (Traditional)".to_string(),
        code if code.starts_with("ja_JP") => "Japanese (Japan)".to_string(),
        code if code.starts_with("ko_KR") => "Korean (Korea)".to_string(),
        code if code.starts_with("ru_RU") => "Russian (Russia)".to_string(),
        code if code.starts_with("it_IT") => "Italian (Italy)".to_string(),
        code if code.starts_with("pt_BR") => "Portuguese (Brazil)".to_string(),
        code if code.starts_with("pt_PT") => "Portuguese (Portugal)".to_string(),
        code if code.starts_with("nl_NL") => "Dutch (Netherlands)".to_string(),
        code if code.starts_with("sv_SE") => "Swedish (Sweden)".to_string(),
        code if code.starts_with("no_NO") => "Norwegian (Norway)".to_string(),
        code if code.starts_with("fi_FI") => "Finnish (Finland)".to_string(),
        code if code.starts_with("pl_PL") => "Polish (Poland)".to_string(),
        code if code.starts_with("cs_CZ") => "Czech (Czech Republic)".to_string(),
        code if code.starts_with("hu_HU") => "Hungarian (Hungary)".to_string(),
        code if code.starts_with("tr_TR") => "Turkish (Turkey)".to_string(),
        code if code.starts_with("ar_SA") => "Arabic (Saudi Arabia)".to_string(),
        code if code.starts_with("hi_IN") => "Hindi (India)".to_string(),
        code if code.starts_with("th_TH") => "Thai (Thailand)".to_string(),
        code if code.starts_with("vi_VN") => "Vietnamese (Vietnam)".to_string(),
        _ => {
            // Fallback: try to extract language and country from locale code
            if let Some(lang_country) = locale_code.split('.').next() {
                if let Some((lang, country)) = lang_country.split_once('_') {
                    format!("{} ({})", lang.to_uppercase(), country.to_uppercase())
                } else {
                    lang_country.to_uppercase()
                }
            } else {
                locale_code.to_string()
            }
        }
    }
}

fn set_locale(locale_code: &str) -> Result<()> {
    let result = Command::new("sudo")
        .args(["localectl", "set-locale", &format!("LANG={}", locale_code)])
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                let display_name = get_available_locales()
                    .iter()
                    .find(|(code, _)| code == locale_code)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| locale_code.to_string());
                notify(&format!("Language set to: {}", display_name));
                Ok(())
            } else {
                notify("Failed to set language (check sudo access)");
                bail!("Failed to set language")
            }
        }
        Err(_) => {
            notify("Failed to set language (check sudo access)");
            bail!("Failed to set language")
        }
    }
}

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut app_state = AppState::new();
    app_state.refresh_status();
    app_state.build_menu();

    loop {
        terminal.draw(|f| {
            let size = f.size();

            // Main container
            let main_block = Block::default()
                .borders(Borders::ALL)
                .title("🌐 Levocale - Locale & Keyboard Switcher")
                .title_alignment(Alignment::Center)
                .border_style(Style::default().fg(Color::Cyan));

            let inner = main_block.inner(size);
            f.render_widget(main_block, size);

            // Split into status, menu area, and instructions
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(4),   // Status display
                    Constraint::Min(0),      // Menu items
                    Constraint::Length(3),   // Instructions
                ])
                .split(inner);

            // Render status section
            let status_block = Block::default()
                .borders(Borders::ALL)
                .title("📊 Current Status")
                .border_style(Style::default().fg(Color::Green));

            let status_text = format!(
                "Locale: {} | Keyboard Layout: {}",
                app_state.current_locale,
                app_state.current_layout
            );

            let status_paragraph = Paragraph::new(status_text)
                .style(Style::default().fg(Color::White))
                .alignment(Alignment::Center)
                .block(status_block);

            f.render_widget(status_paragraph, chunks[0]);

            // Calculate visible area for menu
            let menu_height = chunks[1].height.saturating_sub(2) as usize; // -2 for borders
            let item_height = 2; // Each item takes 2 lines
            let visible_items = menu_height / item_height;

            // Update scroll based on actual visible area
            app_state.adjust_scroll_for_height(visible_items);

            // Get visible menu items
            let end_index = (app_state.scroll_offset + visible_items).min(app_state.menu_items.len());
            let visible_menu_items = if app_state.menu_items.is_empty() {
                &[]
            } else {
                &app_state.menu_items[app_state.scroll_offset..end_index]
            };

            // Menu area
            let menu_block = Block::default()
                .borders(Borders::ALL)
                .title("📋 Options")
                .border_style(Style::default().fg(Color::Blue));

            let menu_inner = menu_block.inner(chunks[1]);
            f.render_widget(menu_block, chunks[1]);

            // Create constraints for visible items
            if !visible_menu_items.is_empty() {
                let menu_constraints: Vec<Constraint> = visible_menu_items
                    .iter()
                    .map(|_| Constraint::Length(item_height as u16))
                    .collect();

                let menu_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(menu_constraints)
                    .split(menu_inner);

                // Render visible menu items
                for (i, item) in visible_menu_items.iter().enumerate() {
                    let global_index = app_state.scroll_offset + i;
                    let is_header = item.label.contains("▼") || item.label.contains("▶");

                    let (style, prefix) = if global_index == app_state.selected {
                        if is_header {
                            (Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD), "► ")
                        } else {
                            (Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD), "► ")
                        }
                    } else if is_header {
                        (Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD), "  ")
                    } else {
                        (Style::default().fg(Color::White), "  ")
                    };

                    let content = format!("{}{}\n{}", prefix, item.label, item.description);

                    let paragraph = Paragraph::new(content)
                        .style(style);

                    if i < menu_chunks.len() {
                        f.render_widget(paragraph, menu_chunks[i]);
                    }
                }
            }

            // Scroll indicators and instructions
            let mut instructions_text = "Controls: ↑/↓ Navigate • Enter Select/Toggle • q/Esc Quit".to_string();

            if app_state.scroll_offset > 0 {
                instructions_text += " • ⬆ More above";
            }
            if end_index < app_state.menu_items.len() {
                instructions_text += " • ⬇ More below";
            }

            let instructions = Paragraph::new(instructions_text)
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::TOP));

            f.render_widget(instructions, chunks[2]);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up => {
                    app_state.move_up();
                }
                KeyCode::Down => {
                    app_state.move_down();
                }
                KeyCode::Enter | KeyCode::Right => {
                    let _ = app_state.execute_selected();
                }
                KeyCode::Left => {
                    // Collapse current section if it's expanded
                    if !app_state.menu_items.is_empty() {
                        let item = &app_state.menu_items[app_state.selected];
                        if (item.label.contains("▼ Keyboard") && app_state.keyboard_section_expanded) ||
                           (item.label.contains("▼ System") && app_state.locale_section_expanded) {
                            app_state.toggle_section();
                        }
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('r') => {
                    // Refresh status
                    app_state.refresh_status();
                    app_state.build_menu();
                }
                _ => {}
            }
        }
    }

    Ok(())
}
