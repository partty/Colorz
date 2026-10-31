//! Screen color picking through the Screenshot portal (GNOME's own picker).

use std::future::Future;

use ashpd::WindowIdentifier;
use ashpd::desktop::ResponseError;
use colorz_core::color::{Color, RgbSpace, chrome_correct};

/// Runs GNOME's color picker and returns the picked sRGB channels in
/// `[0, 1]`, or `None` when the user cancelled. `parent` is the window the
/// picker belongs to, if one is shown.
pub fn pick_color(
    parent: Option<&gtk::Window>,
) -> impl Future<Output = Result<Option<[f64; 3]>, String>> + use<> {
    let parent = parent.cloned();
    async move {
        super::portals_ready().await;
        let identifier = match &parent {
            Some(w) => WindowIdentifier::from_native(w).await,
            None => None,
        };
        let response = ashpd::desktop::Color::pick()
            .identifier(identifier)
            .send()
            .await
            .and_then(|request| request.response());
        match response {
            Ok(c) => Ok(Some([c.red(), c.green(), c.blue()])),
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(None),
            // "Ended in some other way" carries no message worth showing, and
            // the picker may report Esc like this; treat it as a cancel, but
            // log it in case it was a real failure.
            Err(ashpd::Error::Response(ResponseError::Other)) => {
                eprintln!("colorz: the color picker ended without a color");
                Ok(None)
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Turns the portal's sRGB channels into a [`Color`], applying the Chrome
/// mid-tone correction when `chrome` is set.
pub fn pick_to_color(rgb: [f64; 3], chrome: bool) -> Color {
    let rgb = if chrome { chrome_correct(rgb) } else { rgb };
    Color::from_rgb(RgbSpace::Srgb, rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_to_color_chrome() {
        let rgb = [55, 119, 246].map(|v| f64::from(v) / 255.0);
        assert_eq!(pick_to_color(rgb, true).to_srgb8(), [0x34, 0x78, 0xF6]);
        assert_eq!(pick_to_color(rgb, false).to_srgb8(), [55, 119, 246]);
        // Every 8-bit value survives the uncorrected path unchanged.
        for v in 0..=255u8 {
            let x = f64::from(v) / 255.0;
            assert_eq!(pick_to_color([x, x, x], false).to_srgb8(), [v; 3]);
        }
    }
}
