use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{
    env,
    error::Error,
    io::{self, BufRead, BufReader},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

#[derive(Debug, PartialEq)]
enum AppMode {
    Normal,
    Searching,
    Running, // used to know when a command is running
}

#[derive(Debug)]
struct App {
    mode: AppMode,
    commands: Vec<String>,
    selected_index: Option<usize>,
    filtered_commands: Vec<usize>,
    search_input: String,
    // for running commands
    command_output: String,
    output_receiver: Option<Receiver<String>>,
    last_update: Instant,
    spinner_state: usize,
    is_windows: bool,
}

impl App {
    fn new() -> App {
        let is_windows = env::consts::OS == "windows";
        // Choose appropriate commands based on OS
        let commands = if is_windows {
            vec![
                "dir".to_string(),
                "echo \"testing\"".to_string(),
                "ipconfig".to_string(),
                "systeminfo".to_string(),
                "whoami".to_string(),
                "tasklist".to_string(),
            ]
        } else {
            vec![
                "ls".to_string(),
                "echo \"testing\"".to_string(),
                "ifconfig".to_string(),
                "uname -a".to_string(),
                "whoami".to_string(),
                "ps aux".to_string(),
            ]
        };
        let filtered_commands = (0..commands.len()).collect();

        App {
            mode: AppMode::Normal,
            commands,
            filtered_commands,
            selected_index: Some(0),
            search_input: String::new(),
            command_output: String::new(),
            output_receiver: None,
            last_update: Instant::now(),
            spinner_state: 0,
            is_windows,
        }
    }

    fn previous(&mut self) {
        if let Some(current) = self.selected_index {
            if !self.filtered_commands.is_empty() {
                let current_position = self
                    .filtered_commands
                    .iter()
                    .position(|&idx| idx == current)
                    .unwrap_or(0);
                if current_position > 0 {
                    self.selected_index = Some(self.filtered_commands[current_position - 1]);
                }
            }
        }
    }

    fn next(&mut self) {
        if let Some(current) = self.selected_index {
            if !self.filtered_commands.is_empty() {
                let current_position = self
                    .filtered_commands
                    .iter()
                    .position(|&idx| idx == current)
                    .unwrap_or(0);

                if current_position < self.filtered_commands.len() - 1 {
                    self.selected_index = Some(self.filtered_commands[current_position + 1]);
                }
            }
        }
    }

    fn update_filter(&mut self) {
        // first store old selection before updating filtered_commands
        let old_selection = self.selected_index;

        // update filtered commands
        self.filtered_commands = self
            .commands
            .iter()
            .enumerate()
            .filter(|(_, cmd)| {
                cmd.to_lowercase()
                    .contains(&self.search_input.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect();

        self.selected_index = if self.filtered_commands.is_empty() {
            // if no results, temporarily remove selection
            None
        } else if let Some(selected) = old_selection {
            if self.filtered_commands.contains(&selected) {
                Some(selected)
            } else {
                self.filtered_commands.first().copied()
            }
        } else {
            self.filtered_commands.first().copied()
        }
    }

    fn is_searching(&self) -> bool {
        self.mode == AppMode::Searching
    }

    fn execute_command(&mut self) -> io::Result<()> {
        if let Some(idx) = self.selected_index {
            let command = &self.commands[idx];

            // handle command creation based on the OS
            let (program, args) = if self.is_windows {
                ("cmd", vec!["/C", command])
            } else {
                let mut parts = command.split_whitespace();
                let cmd = parts.next().unwrap_or("");
                let cmd_args: Vec<&str> = parts.collect();
                (cmd, cmd_args)
            };

            let mut child = Command::new(program)
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;

            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();

            let (tx, rx) = mpsc::channel();
            self.output_receiver = Some(rx);

            let tx_clone = tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let _ = tx_clone.send(line);
                    }
                }
            });

            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let _ = tx.send(format!("Error: {}", line));
                    }
                }
            });

            self.mode = AppMode::Running;
            self.command_output.clear();
        }

        Ok(())
    }

    fn update_spinner(&mut self) {
        if Instant::now().duration_since(self.last_update) >= Duration::from_millis(100) {
            self.spinner_state = (self.spinner_state + 1) % 4;
            self.last_update = Instant::now()
        }
    }

    fn get_spinner_char(&self) -> &str {
        match self.spinner_state {
            0 => "⠋",
            1 => "⠙",
            2 => "⠹",
            3 => "⠸",
            _ => "⠋",
        }
    }

    fn check_command_output(&mut self) {
        if let Some(ref receiver) = self.output_receiver {
            while let Ok(line) = receiver.try_recv() {
                self.command_output.push_str(&line);
                self.command_output.push('\n');
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // create app state
    let mut app = App::new();

    loop {
        if app.mode == AppMode::Running {
            app.update_spinner();
            app.check_command_output();
        }

        terminal.draw(|frame| ui(frame, &app))?;

        // handle events
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match app.mode {
                        AppMode::Normal => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                // close the app
                                if !app.search_input.is_empty() {
                                    app.search_input.clear();
                                    app.update_filter();
                                } else {
                                    break;
                                }
                            }
                            KeyCode::Char('j') | KeyCode::Down => app.next(),
                            KeyCode::Char('k') | KeyCode::Up => app.previous(),
                            KeyCode::Char('/') => {
                                app.mode = AppMode::Searching;
                                // app.search_input.clear();
                            }
                            KeyCode::Enter => {
                                let _ = app.execute_command();
                            }
                            _ => {}
                        },
                        AppMode::Searching => match key.code {
                            KeyCode::Esc => {
                                app.mode = AppMode::Normal;
                                app.search_input.clear();
                                app.update_filter();
                                // if no selection after clearing search, select the first item
                                if app.selected_index.is_none() && !app.filtered_commands.is_empty()
                                {
                                    app.selected_index = Some(app.filtered_commands[0]);
                                }
                            }
                            KeyCode::Enter => {
                                app.mode = AppMode::Normal;
                            }
                            KeyCode::Char(c) => {
                                app.search_input.push(c);
                                app.update_filter();
                            }
                            KeyCode::Backspace => {
                                app.search_input.pop();
                                app.update_filter();
                            }
                            _ => {}
                        },
                        AppMode::Running => match key.code {
                            KeyCode::Esc => {
                                app.mode = AppMode::Normal;
                                app.output_receiver = None;
                            }
                            _ => {}
                        },
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
    )?;

    terminal.show_cursor()?;

    Ok(())
}

fn ui(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(2, 3),
            Constraint::Length(3),
        ])
        .split(area);

    let search_block = Block::default()
        .title("Search (press '/' to search, 'enter' to navigate in the results)")
        .borders(Borders::ALL);

    let search_text = format!("/{}", app.search_input);

    frame.render_widget(Paragraph::new(search_text).block(search_block), layout[0]);

    let items: Vec<ListItem> = app
        .filtered_commands
        .iter()
        .map(|&index| {
            let command = &app.commands[index];
            let display_text = if app.mode == AppMode::Running && Some(index) == app.selected_index
            {
                format!("{} {} (running...)", command, app.get_spinner_char())
            } else {
                command.clone()
            };
            ListItem::new(display_text)
        })
        .collect();

    // create list widget
    let list = List::new(items)
        .block(Block::default().title("Commands").borders(Borders::ALL))
        .highlight_style(Style::default().blue())
        .highlight_symbol(">> ");

    frame.render_stateful_widget(
        list,
        layout[1],
        &mut ratatui::widgets::ListState::default().with_selected(
            app.filtered_commands
                .iter()
                .position(|&idx| Some(idx) == app.selected_index),
        ),
    );
    // command output
    let output_block = Block::default().title("Output").borders(Borders::ALL);

    frame.render_widget(
        Paragraph::new(app.command_output.as_str())
            .block(output_block)
            .wrap(Wrap { trim: true }),
        layout[2],
    );

    // debug
    let debug_block = Block::default().title("debug").borders(Borders::ALL);
    let debug_text = format!(
        "Selected index: {}, filtered: {:?}, search_input: {}",
        app.selected_index.unwrap_or(20),
        app.filtered_commands,
        app.search_input
    );
    frame.render_widget(Paragraph::new(debug_text).block(debug_block), layout[3]);
    // end debug
}
