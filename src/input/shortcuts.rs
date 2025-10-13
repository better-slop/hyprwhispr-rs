use anyhow::{Context, Result};
use evdev::{Device, InputEventKind, Key};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct ShortcutEvent {
    pub triggered_at: Instant,
}

pub struct GlobalShortcuts {
    devices: Vec<Device>,
    target_keys: HashSet<Key>,
    shortcut_name: String,
}

impl GlobalShortcuts {
    pub fn new(shortcut: &str) -> Result<Self> {
        let target_keys = Self::parse_shortcut(shortcut)?;
        let devices = Self::find_keyboard_devices()?;

        if devices.is_empty() {
            return Err(anyhow::anyhow!("No keyboard devices found"));
        }

        info!(
            "Global shortcuts initialized - monitoring {} device(s) for: {}",
            devices.len(),
            shortcut
        );
        debug!("Target keys: {:?}", target_keys);

        Ok(Self {
            devices,
            target_keys,
            shortcut_name: shortcut.to_string(),
        })
    }

    pub fn run(mut self, tx: mpsc::Sender<ShortcutEvent>, stop: Arc<AtomicBool>) -> Result<()> {
        let mut pressed_keys: HashSet<Key> = HashSet::new();
        let mut last_trigger = Instant::now() - Duration::from_secs(10);
        let debounce_duration = Duration::from_millis(500);

        info!("🎯 Listening for shortcut: {}", self.shortcut_name);

        loop {
            if stop.load(Ordering::Relaxed) {
                info!("Stopping shortcut listener: {}", self.shortcut_name);
                break;
            }
            // Check each device
            let target_keys = &self.target_keys;
            let shortcut_name = &self.shortcut_name;

            for device in &mut self.devices {
                // Fetch events from this device
                match device.fetch_events() {
                    Ok(events) => {
                        for event in events {
                            match event.kind() {
                                InputEventKind::Key(key) => {
                                    let value = event.value();

                                    match value {
                                        // Key pressed
                                        1 => {
                                            debug!("Key pressed: {:?}", key);
                                            pressed_keys.insert(key);

                                            // Check if target combination is pressed
                                            if target_keys.is_subset(&pressed_keys) {
                                                let now = Instant::now();

                                                // Debounce: only trigger if enough time has passed
                                                if now.duration_since(last_trigger)
                                                    > debounce_duration
                                                {
                                                    info!(
                                                        "✨ Shortcut triggered: {}",
                                                        shortcut_name
                                                    );
                                                    last_trigger = now;

                                                    // Send event (non-blocking)
                                                    if let Err(e) = tx.try_send(ShortcutEvent {
                                                        triggered_at: now,
                                                    }) {
                                                        warn!(
                                                            "Failed to send shortcut event: {}",
                                                            e
                                                        );
                                                    }
                                                } else {
                                                    debug!("Shortcut debounced (too soon)");
                                                }
                                            }
                                        }
                                        // Key released
                                        0 => {
                                            debug!("Key released: {:?}", key);
                                            pressed_keys.remove(&key);
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::WouldBlock {
                            error!("Error fetching events: {}", e);
                        }
                    }
                }
            }

            // Small sleep to prevent busy-waiting
            std::thread::sleep(Duration::from_millis(10));
        }

        Ok(())
    }

    fn is_target_combination(&self, pressed: &HashSet<Key>) -> bool {
        // Check if all target keys are pressed
        self.target_keys.is_subset(pressed)
    }

    fn parse_shortcut(shortcut: &str) -> Result<HashSet<Key>> {
        let mut keys = HashSet::new();

        for part in shortcut.split('+') {
            let part = part.trim().to_uppercase();
            let key =
                Self::parse_key(&part).with_context(|| format!("Failed to parse key: {}", part))?;
            keys.insert(key);
        }

        if keys.is_empty() {
            return Err(anyhow::anyhow!("Empty shortcut"));
        }

        Ok(keys)
    }

    fn parse_key(key_str: &str) -> Result<Key> {
        match key_str {
            // Modifiers
            "SUPER" | "META" | "WIN" | "WINDOWS" => Ok(Key::KEY_LEFTMETA),
            "ALT" => Ok(Key::KEY_LEFTALT),
            "CTRL" | "CONTROL" => Ok(Key::KEY_LEFTCTRL),
            "SHIFT" => Ok(Key::KEY_LEFTSHIFT),

            // Function keys
            "F1" => Ok(Key::KEY_F1),
            "F2" => Ok(Key::KEY_F2),
            "F3" => Ok(Key::KEY_F3),
            "F4" => Ok(Key::KEY_F4),
            "F5" => Ok(Key::KEY_F5),
            "F6" => Ok(Key::KEY_F6),
            "F7" => Ok(Key::KEY_F7),
            "F8" => Ok(Key::KEY_F8),
            "F9" => Ok(Key::KEY_F9),
            "F10" => Ok(Key::KEY_F10),
            "F11" => Ok(Key::KEY_F11),
            "F12" => Ok(Key::KEY_F12),

            // Letter keys
            "A" => Ok(Key::KEY_A),
            "B" => Ok(Key::KEY_B),
            "C" => Ok(Key::KEY_C),
            "D" => Ok(Key::KEY_D),
            "E" => Ok(Key::KEY_E),
            "F" => Ok(Key::KEY_F),
            "G" => Ok(Key::KEY_G),
            "H" => Ok(Key::KEY_H),
            "I" => Ok(Key::KEY_I),
            "J" => Ok(Key::KEY_J),
            "K" => Ok(Key::KEY_K),
            "L" => Ok(Key::KEY_L),
            "M" => Ok(Key::KEY_M),
            "N" => Ok(Key::KEY_N),
            "O" => Ok(Key::KEY_O),
            "P" => Ok(Key::KEY_P),
            "Q" => Ok(Key::KEY_Q),
            "R" => Ok(Key::KEY_R),
            "S" => Ok(Key::KEY_S),
            "T" => Ok(Key::KEY_T),
            "U" => Ok(Key::KEY_U),
            "V" => Ok(Key::KEY_V),
            "W" => Ok(Key::KEY_W),
            "X" => Ok(Key::KEY_X),
            "Y" => Ok(Key::KEY_Y),
            "Z" => Ok(Key::KEY_Z),

            // Number keys
            "0" => Ok(Key::KEY_0),
            "1" => Ok(Key::KEY_1),
            "2" => Ok(Key::KEY_2),
            "3" => Ok(Key::KEY_3),
            "4" => Ok(Key::KEY_4),
            "5" => Ok(Key::KEY_5),
            "6" => Ok(Key::KEY_6),
            "7" => Ok(Key::KEY_7),
            "8" => Ok(Key::KEY_8),
            "9" => Ok(Key::KEY_9),

            // Special keys
            "SPACE" => Ok(Key::KEY_SPACE),
            "ENTER" | "RETURN" => Ok(Key::KEY_ENTER),
            "ESC" | "ESCAPE" => Ok(Key::KEY_ESC),
            "TAB" => Ok(Key::KEY_TAB),
            "BACKSPACE" => Ok(Key::KEY_BACKSPACE),
            "DELETE" | "DEL" => Ok(Key::KEY_DELETE),
            "INSERT" | "INS" => Ok(Key::KEY_INSERT),
            "HOME" => Ok(Key::KEY_HOME),
            "END" => Ok(Key::KEY_END),
            "PAGEUP" | "PGUP" => Ok(Key::KEY_PAGEUP),
            "PAGEDOWN" | "PGDOWN" => Ok(Key::KEY_PAGEDOWN),

            // Arrow keys
            "UP" => Ok(Key::KEY_UP),
            "DOWN" => Ok(Key::KEY_DOWN),
            "LEFT" => Ok(Key::KEY_LEFT),
            "RIGHT" => Ok(Key::KEY_RIGHT),

            _ => Err(anyhow::anyhow!("Unknown key: {}", key_str)),
        }
    }

    fn find_keyboard_devices() -> Result<Vec<Device>> {
        let mut keyboards = Vec::new();

        for (path, device) in evdev::enumerate() {
            // Check if device supports keyboard events
            if let Some(keys) = device.supported_keys() {
                // Verify it has typical keyboard keys
                if keys.contains(Key::KEY_A)
                    && keys.contains(Key::KEY_S)
                    && keys.contains(Key::KEY_D)
                {
                    let name = device.name().unwrap_or("Unknown");
                    info!("Found keyboard device: {} at {:?}", name, path);
                    keyboards.push(device);
                }
            }
        }

        if keyboards.is_empty() {
            warn!("No keyboard devices found!");
            warn!("Make sure you have read permissions for /dev/input/event*");
            warn!("You may need to add your user to the 'input' group");
        }

        Ok(keyboards)
    }

    pub fn list_available_keyboards() -> Result<Vec<(PathBuf, String)>> {
        let mut keyboards = Vec::new();

        for (path, device) in evdev::enumerate() {
            if let Some(keys) = device.supported_keys() {
                if keys.contains(Key::KEY_A) && keys.contains(Key::KEY_ENTER) {
                    let name = device.name().unwrap_or("Unknown").to_string();
                    keyboards.push((path, name));
                }
            }
        }

        Ok(keyboards)
    }
}
