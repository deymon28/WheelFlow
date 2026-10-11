use std::{env, fs, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub speed: u32,
    pub duration: u32,
    pub enabled: bool,
    pub acceleration: bool,
    pub fullscreen_bypass: bool,
    pub run_as_admin: bool,
    pub exclusions: Vec<String>,
}

pub const SPEED: std::ops::RangeInclusive<u32> = 50..=200;
pub const DURATION: std::ops::RangeInclusive<u32> = 60..=600;

impl Default for Config {
    fn default() -> Self {
        Self {
            speed: 100,
            duration: 250,
            enabled: true,
            acceleration: true,
            fullscreen_bypass: true,
            run_as_admin: false,
            exclusions: Vec::new(),
        }
    }
}

impl Config {
    fn path() -> Result<PathBuf, String> {
        env::var_os("APPDATA")
            .map(|p| PathBuf::from(p).join("WheelFlow").join("settings.ini"))
            .ok_or_else(|| "The application data folder is unavailable.".to_string())
    }

    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        let mut result = Self::default();
        for line in text.trim_start_matches('\u{feff}').lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "speed" => {
                    if let Ok(n) = value.parse::<u32>() {
                        result.speed = n.clamp(*SPEED.start(), *SPEED.end());
                    }
                }
                "duration" => {
                    if let Ok(n) = value.parse::<u32>() {
                        result.duration = n.clamp(*DURATION.start(), *DURATION.end());
                    }
                }
                "enabled" => result.enabled = value != "false",
                "acceleration" => result.acceleration = value != "false",
                "fullscreen_bypass" => result.fullscreen_bypass = value != "false",
                "run_as_admin" => result.run_as_admin = value == "true",
                "exclude" => result.exclusions = Self::exclusion_names(value),
                _ => (),
            }
        }
        result
    }

    pub fn exclusion_names(text: &str) -> Vec<String> {
        let mut names = Vec::new();
        for raw in text.split([',', '\r', '\n']) {
            let name = raw
                .trim()
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_lowercase();
            let name = name.strip_suffix(".exe").unwrap_or(&name).trim();
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
        names
    }

    fn serialize(&self) -> String {
        format!(
            "speed={}\nduration={}\nenabled={}\nacceleration={}\nfullscreen_bypass={}\nrun_as_admin={}\nexclude={}\n",
            self.speed.clamp(*SPEED.start(), *SPEED.end()),
            self.duration.clamp(*DURATION.start(), *DURATION.end()),
            self.enabled,
            self.acceleration,
            self.fullscreen_bypass,
            self.run_as_admin,
            Self::exclusion_names(&self.exclusions.join(",")).join(",")
        )
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path()?;
        fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| format!("Could not create settings folder: {e}"))?;
        // Write a complete temporary file first so an interrupted save cannot
        // leave a truncated settings file behind.
        let temporary = path.with_extension("ini.tmp");
        fs::write(&temporary, self.serialize())
            .and_then(|_| fs::rename(&temporary, &path))
            .map_err(|e| format!("Could not save settings: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_bom_and_preserves_preferences() {
        let c = Config::parse(
            "\u{feff}speed=125\r\nduration=250\r\nenabled=false\r\nexclude=Chrome.exe,game\r\n",
        );
        assert_eq!((c.speed, c.duration, c.enabled), (125, 250, false));
        assert_eq!(c.exclusions, vec!["chrome", "game"]);
    }

    #[test]
    fn invalid_values_default_and_out_of_range_values_clamp() {
        let c = Config::parse("speed=no\nduration=0\nunknown=1\n");
        assert_eq!((c.speed, c.duration), (100, 60));
        assert_eq!(Config::parse("speed=999\nduration=999").speed, 200);
        assert_eq!(Config::parse("speed=999\nduration=999").duration, 600);
    }

    #[test]
    fn new_options_default_on_and_round_trip() {
        let legacy = Config::parse("speed=80\nduration=100\nenabled=true\nexclude=\n");
        assert!(legacy.acceleration && legacy.fullscreen_bypass && !legacy.run_as_admin);
        let mut admin = Config::default();
        admin.run_as_admin = true;
        assert!(Config::parse(&admin.serialize()).run_as_admin);
        assert_eq!((legacy.speed, legacy.duration), (80, 100));
        let mut c = Config::default();
        c.acceleration = false;
        c.fullscreen_bypass = false;
        let parsed = Config::parse(&c.serialize());
        assert!(!parsed.acceleration && !parsed.fullscreen_bypass);
    }

    #[test]
    fn exclusions_normalize_and_round_trip() {
        let mut c = Config::default();
        c.exclusions = Config::exclusion_names(" Chrome.EXE\r\nchrome\nD:\\Games\\Game.exe,editor ");
        assert_eq!(c.exclusions, vec!["chrome", "game", "editor"]);
        assert_eq!(Config::parse(&c.serialize()).exclusions, c.exclusions);
    }
}
