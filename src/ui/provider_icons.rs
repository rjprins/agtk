use std::path::Path;

use gtk::prelude::*;

use crate::control::SessionKind;
use crate::providers::AgentProvider;

#[derive(Clone, Copy)]
enum ProviderIcon {
    Zsh,
    Claude,
    Codex,
    Gemini,
    Grok,
    Custom,
}

impl ProviderIcon {
    const fn svg(self) -> &'static [u8] {
        match self {
            Self::Zsh => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/data/provider-zsh.svg"
            )),
            Self::Claude => {
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/data/provider-claude.svg"
                ))
            }
            Self::Codex => {
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/data/provider-codex.svg"
                ))
            }
            Self::Gemini => {
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/data/provider-gemini.svg"
                ))
            }
            Self::Grok => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/data/provider-grok.svg"
            )),
            Self::Custom => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/data/provider-custom.svg"
            )),
        }
    }

    const fn tooltip(self) -> &'static str {
        match self {
            Self::Zsh => "zsh shell",
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::Grok => "Grok",
            Self::Custom => "Custom terminal",
        }
    }
}

pub(super) fn session_icon(kind: SessionKind, program: &Path) -> gtk::Image {
    let icon = match kind {
        SessionKind::Shell => ProviderIcon::Zsh,
        SessionKind::Codex => ProviderIcon::Codex,
        SessionKind::Claude => ProviderIcon::Claude,
        SessionKind::Gemini => ProviderIcon::Gemini,
        SessionKind::Custom => match program.file_stem().and_then(|name| name.to_str()) {
            Some(name) if name.eq_ignore_ascii_case("grok") => ProviderIcon::Grok,
            _ => ProviderIcon::Custom,
        },
    };
    image(icon)
}

pub(super) fn agent_icon(provider: AgentProvider) -> gtk::Image {
    image(match provider {
        AgentProvider::Codex => ProviderIcon::Codex,
        AgentProvider::Claude => ProviderIcon::Claude,
    })
}

pub(super) fn brand_icon() -> gtk::Image {
    let bytes = glib::Bytes::from_static(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/galaxy.png"
    )));
    let texture = gtk::gdk::Texture::from_bytes(&bytes).expect("embedded brand icon is valid");
    let image = gtk::Image::from_paintable(Some(&texture));
    image.set_pixel_size(22);
    image.set_valign(gtk::Align::Center);
    image.set_tooltip_text(Some("agtk"));
    image
}

fn image(icon: ProviderIcon) -> gtk::Image {
    let bytes = glib::Bytes::from_static(icon.svg());
    let texture = gtk::gdk::Texture::from_bytes(&bytes).expect("embedded provider icon is valid");
    let image = gtk::Image::from_paintable(Some(&texture));
    // No pixel size: the stylesheet sizes these so they follow the UI text size.
    image.add_css_class("provider-icon");
    image.set_tooltip_text(Some(icon.tooltip()));
    image
}
