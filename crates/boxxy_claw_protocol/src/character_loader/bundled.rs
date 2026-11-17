//! Compile-time embedded default character definitions and avatars.

pub(super) struct BundledCharacter {
    pub(super) slug: &'static str,
    pub(super) default_color: &'static str,
    pub(super) toml_bytes: &'static [u8],
    pub(super) avatar_bytes: &'static [u8],
}

pub(super) const BUNDLED_JSON: &[u8] =
    include_bytes!("../../../../resources/characters/characters.json");

const INCOGNITO_AVATAR: &[u8] =
    include_bytes!("../../../../resources/characters/inco-gnito/AVATAR.png");

pub(super) const BUNDLED_CHARACTERS: &[BundledCharacter] = &[
    BundledCharacter {
        slug: "niko-una",
        default_color: "#8035D4",
        toml_bytes: include_bytes!("../../../../resources/characters/niko-una/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../../resources/characters/niko-una/AVATAR.png"),
    },
    BundledCharacter {
        slug: "levi-kujo",
        default_color: "#204A87",
        toml_bytes: include_bytes!("../../../../resources/characters/levi-kujo/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../../resources/characters/levi-kujo/AVATAR.png"),
    },
    BundledCharacter {
        slug: "kuro",
        default_color: "#B31825",
        toml_bytes: include_bytes!("../../../../resources/characters/kuro/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../../resources/characters/kuro/AVATAR.png"),
    },
];

pub(super) fn find_bundled(slug: &str) -> Option<&'static BundledCharacter> {
    BUNDLED_CHARACTERS.iter().find(|b| b.slug == slug)
}

/// Returns the bundled 256×256 PNG avatar bytes for the `/incognito` persona (`Inco Gnito`).
#[must_use]
pub const fn incognito_avatar_bytes() -> &'static [u8] {
    INCOGNITO_AVATAR
}

/// Returns the bundled default PNG avatar bytes for the three built-in characters
/// (`niko-una`, `levi-kujo`, `kuro`), or `None` for user-created characters.
#[must_use]
pub fn bundled_default_avatar(slug: &str) -> Option<&'static [u8]> {
    find_bundled(slug).map(|b| b.avatar_bytes)
}

/// Returns the bundled default hex color for the three built-in characters
/// (`niko-una`, `levi-kujo`, `kuro`), or `None` for user-created characters.
#[must_use]
pub fn bundled_default_color(slug: &str) -> Option<&'static str> {
    find_bundled(slug).map(|b| b.default_color)
}
