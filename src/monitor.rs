use crate::config::Config;
use crate::ipc::{connect_to_daemon, read_ups_data};
use crate::ups_data::UpsData;
use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{self, ClearType},
};
use std::io::{stdout, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn run_monitor(config: &Config) -> Result<()> {
    let mut reader = connect_to_daemon(&config.ipc.socket_path)?;

    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = monitor_loop(&mut stdout, &mut reader, config.battery.min_valid_voltage);

    // Cleanup terminal (always, even on error)
    let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();

    result
}

fn monitor_loop(
    stdout: &mut impl Write,
    reader: &mut BufReader<UnixStream>,
    min_valid_voltage: u32,
) -> Result<()> {
    loop {
        // Check for quit key (non-blocking)
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(KeyEvent { code, .. }) = event::read()? {
                match code {
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => break,
                    KeyCode::Char('c')
                        if event::read()? == Event::Key(KeyEvent::from(KeyCode::Char('c'))) =>
                    {
                        break
                    }
                    _ => {}
                }
            }
        }

        // Try to read data
        match read_ups_data(reader) {
            Ok(ups) => {
                render_monitor(stdout, &ups, min_valid_voltage)?;
            }
            Err(_) => {
                // Connection lost, try to show error
                execute!(
                    stdout,
                    cursor::MoveTo(0, 0),
                    terminal::Clear(ClearType::All)
                )?;
                writeln!(stdout, "\x1b[31mConnection to daemon lost.\x1b[0m")?;
                writeln!(stdout, "Press 'q' to exit.")?;
                stdout.flush()?;
            }
        }
    }

    Ok(())
}

fn render_monitor(stdout: &mut impl Write, ups: &UpsData, min_valid_voltage: u32) -> Result<()> {
    execute!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal::Clear(ClearType::All)
    )?;

    let on_battery = ups.is_on_battery(min_valid_voltage);
    let power_icon = if on_battery { "BATTERY" } else { "GRID" };

    let charging_status = match ups.cs {
        2 => " [CHARGING]",
        3 => " [FULL]",
        _ if ups.ba > 0 => " [CHARGING]",
        _ if ups.ba < 0 => " [DISCHARGING]",
        _ => "",
    };

    // Header
    writeln!(stdout, "\x1b[1;36m=== Web3 Pi UPS Monitor ===\x1b[0m")?;
    writeln!(stdout, "Press 'q' or ESC to exit")?;
    writeln!(stdout)?;

    // Battery bar
    let bar_width = 30;
    let filled = (ups.soc as usize * bar_width) / 100;
    let bar_color = match ups.soc {
        0..=20 => "\x1b[31m",  // Red
        21..=50 => "\x1b[33m", // Yellow
        _ => "\x1b[32m",       // Green
    };

    write!(stdout, "Battery: {}[", bar_color)?;
    write!(stdout, "{}", "=".repeat(filled))?;
    write!(stdout, "{}", " ".repeat(bar_width - filled))?;
    writeln!(stdout, "]\x1b[0m {}%{}", ups.soc, charging_status)?;
    writeln!(stdout)?;

    // Power source with color
    let power_color = if on_battery { "\x1b[33m" } else { "\x1b[32m" };
    writeln!(stdout, "Power:   {}[{}]\x1b[0m", power_color, power_icon)?;
    writeln!(stdout)?;

    // Details
    writeln!(stdout, "\x1b[1mDetails:\x1b[0m")?;
    writeln!(
        stdout,
        "  Battery Voltage:  {:.2} V",
        ups.battery_voltage_v()
    )?;
    writeln!(stdout, "  Battery Current:  {} mA", ups.ba)?;
    writeln!(stdout, "  Input Voltage:    {:.2} V", ups.input_voltage_v())?;
    writeln!(stdout, "  Temperature:      {:.1} °C", ups.temperature_c())?;
    writeln!(stdout, "  Uptime:           {} sec", ups.up)?;

    // Power good indicator
    let pg_color = if ups.pg == 1 { "\x1b[32m" } else { "\x1b[31m" };
    let pg_status = if ups.pg == 1 { "OK" } else { "NO" };
    writeln!(
        stdout,
        "  Power Good:       {}[{}]\x1b[0m",
        pg_color, pg_status
    )?;

    stdout.flush()?;
    Ok(())
}
