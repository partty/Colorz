//! The "copy as" choices shared by the Pick tab and Settings.

use std::path::Path;

use colorz_core::storage::{Config, TargetChoice};

/// The "copy as" choices: the built-in spaces, the remembered ICC files, and
/// the current target's file if it is not remembered.
pub fn target_choices(cfg: &Config) -> Vec<TargetChoice> {
    let mut v = vec![
        TargetChoice::Srgb,
        TargetChoice::DisplayP3,
        TargetChoice::AdobeRgb,
    ];
    for path in &cfg.custom_icc {
        let choice = TargetChoice::Icc { path: path.clone() };
        if !v.contains(&choice) {
            v.push(choice);
        }
    }
    if matches!(cfg.target, TargetChoice::Icc { .. }) && !v.contains(&cfg.target) {
        v.push(cfg.target.clone());
    }
    v
}

/// Labels for `choices`; ICC files that share a name get their parent
/// directory appended.
pub fn target_labels(choices: &[TargetChoice]) -> Vec<String> {
    let names: Vec<String> = choices.iter().map(target_label).collect();
    choices
        .iter()
        .zip(&names)
        .map(|(choice, name)| match choice {
            TargetChoice::Icc { path } if names.iter().filter(|n| *n == name).count() > 1 => {
                let parent = path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                format!("{name} ({parent})")
            }
            _ => name.clone(),
        })
        .collect()
}

fn target_label(choice: &TargetChoice) -> String {
    match choice {
        TargetChoice::Srgb => "sRGB".to_owned(),
        TargetChoice::DisplayP3 => "Display P3".to_owned(),
        TargetChoice::AdobeRgb => "Adobe RGB (1998)".to_owned(),
        TargetChoice::Icc { path } => Path::new(path).file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_choices_dedupe_and_disambiguate() {
        let cfg = Config {
            custom_icc: vec!["/a/x.icc".into(), "/a/x.icc".into(), "/b/x.icc".into()],
            ..Config::default()
        };
        let choices = target_choices(&cfg);
        assert_eq!(choices.len(), 5);
        let labels = target_labels(&choices);
        assert_eq!(labels[3], "x.icc (/a)");
        assert_eq!(labels[4], "x.icc (/b)");
        assert_eq!(labels[0], "sRGB");
    }

    #[test]
    fn target_choices_list_remembered_files() {
        let icc = TargetChoice::Icc {
            path: "/p/printer.icc".into(),
        };
        let cfg = Config {
            custom_icc: vec!["/p/a.icc".into()],
            target: icc.clone(),
            ..Config::default()
        };
        let choices = target_choices(&cfg);
        assert_eq!(choices.len(), 5);
        assert_eq!(choices[4], icc);
        assert_eq!(target_label(&choices[3]), "a.icc");
        assert_eq!(target_label(&choices[2]), "Adobe RGB (1998)");
    }

    #[test]
    fn target_choices_include_custom() {
        let cfg = Config {
            custom_icc: vec!["/p/a.icc".into()],
            ..Config::default()
        };
        let choices = target_choices(&cfg);
        assert_eq!(
            choices[..3],
            [
                TargetChoice::Srgb,
                TargetChoice::DisplayP3,
                TargetChoice::AdobeRgb
            ]
        );
        assert_eq!(
            choices[3],
            TargetChoice::Icc {
                path: "/p/a.icc".into()
            }
        );
    }
}
