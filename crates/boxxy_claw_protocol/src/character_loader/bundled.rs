//! Compile-time embedded default character definitions, v2 manifests, avatars, and `16:9` portraits.

pub(super) struct BundledCharacter {
    pub(super) slug: &'static str,
    pub(super) version: (u32, u32, u32),
    pub(super) default_color: &'static str,
    pub(super) manifest_bytes: &'static [u8],
    pub(super) toml_bytes: &'static [u8],
    pub(super) avatar_bytes: &'static [u8],
    pub(super) portrait_bytes: &'static [u8],
    pub(super) legacy_v1_avatar: (u64, u64),
    pub(super) default_greetings: &'static [&'static str],
}

pub(super) const BUNDLED_JSON: &[u8] =
    include_bytes!("../../../../resources/characters/characters.json");

const INCOGNITO_AVATAR: &[u8] =
    include_bytes!("../../../../resources/characters/inco-gnito/AVATAR.png");

const INCOGNITO_PORTRAIT: &[u8] =
    include_bytes!("../../../../resources/characters/inco-gnito/portrait.webp");

pub(super) const BUNDLED_CHARACTERS: &[BundledCharacter] = &[
    BundledCharacter {
        slug: "niko-una",
        version: (2, 0, 1),
        default_color: "#8035D4",
        manifest_bytes: include_bytes!("../../../../resources/characters/niko-una/manifest.toml"),
        toml_bytes: include_bytes!("../../../../resources/characters/niko-una/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../../resources/characters/niko-una/AVATAR.png"),
        portrait_bytes: include_bytes!("../../../../resources/characters/niko-una/portrait.webp"),
        legacy_v1_avatar: (118_047, 0x427d_3509_5d56_73ea),
        default_greetings: &[
            "I-it's not like I pre-warmed the terminal for you or anything, Master!",
            "Hmph. Show me what broke so I can clean it up properly.",
            "Everything's tidy so far. Try not to make a mess while I'm watching.",
        ],
    },
    BundledCharacter {
        slug: "levi-kujo",
        version: (2, 0, 1),
        default_color: "#204A87",
        manifest_bytes: include_bytes!("../../../../resources/characters/levi-kujo/manifest.toml"),
        toml_bytes: include_bytes!("../../../../resources/characters/levi-kujo/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../../resources/characters/levi-kujo/AVATAR.png"),
        portrait_bytes: include_bytes!("../../../../resources/characters/levi-kujo/portrait.webp"),
        legacy_v1_avatar: (121_158, 0x44dd_38fe_3b12_8358),
        default_greetings: &[
            "State the objective. Keep it brief.",
            "Shell is ready. Show me the failing trace.",
            "No filler. Point me at the bottleneck.",
        ],
    },
    BundledCharacter {
        slug: "kuro",
        version: (2, 0, 1),
        default_color: "#B31825",
        manifest_bytes: include_bytes!("../../../../resources/characters/kuro/manifest.toml"),
        toml_bytes: include_bytes!("../../../../resources/characters/kuro/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../../resources/characters/kuro/AVATAR.png"),
        portrait_bytes: include_bytes!("../../../../resources/characters/kuro/portrait.webp"),
        legacy_v1_avatar: (65_320, 0xad33_6a11_e9ee_480c),
        default_greetings: &[
            "The AUR gods await our next compilation, btw.",
            "Speak thy command — preferably something worthy of a custom kernel.",
            "Another terminal summoned from the void. Let us consult the sacred Wiki.",
        ],
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

/// Returns the bundled 768×432 (`16:9`) WebP portrait bytes for the `/incognito` persona (`Inco Gnito`).
#[must_use]
pub const fn incognito_portrait_bytes() -> &'static [u8] {
    INCOGNITO_PORTRAIT
}

/// Returns the bundled default PNG avatar bytes for the three built-in characters
/// (`niko-una`, `levi-kujo`, `kuro`), or `None` for user-created characters.
#[must_use]
pub fn bundled_default_avatar(slug: &str) -> Option<&'static [u8]> {
    find_bundled(slug).map(|b| b.avatar_bytes)
}

/// Returns the bundled default 768×432 (`16:9`) WebP portrait bytes for the three built-in
/// characters (`niko-una`, `levi-kujo`, `kuro`), or `None` for user-created characters.
#[must_use]
pub fn bundled_default_portrait(slug: &str) -> Option<&'static [u8]> {
    find_bundled(slug).map(|b| b.portrait_bytes)
}

/// Returns the bundled default `manifest.toml` bytes for the three built-in characters.
#[must_use]
pub fn bundled_default_manifest(slug: &str) -> Option<&'static [u8]> {
    find_bundled(slug).map(|b| b.manifest_bytes)
}

/// Returns the bundled default hex color for the three built-in characters
/// (`niko-una`, `levi-kujo`, `kuro`), or `None` for user-created characters.
#[must_use]
pub fn bundled_default_color(slug: &str) -> Option<&'static str> {
    find_bundled(slug).map(|b| b.default_color)
}

/// Returns fallback default greetings for the three built-in characters when an existing
/// pre-v2 `CHARACTER.toml` on disk does not yet define a `greetings` array.
#[must_use]
pub fn bundled_default_greetings(slug: &str) -> Option<&'static [&'static str]> {
    find_bundled(slug).map(|b| b.default_greetings)
}

/// Checks whether `disk_bytes` matches the unmodified v1 placeholder `AVATAR.png` for `slug`.
#[must_use]
pub(super) fn matches_legacy_v1_bundled_avatar(slug: &str, disk_bytes: &[u8]) -> bool {
    let Some((expected_len, expected_hash)) = find_bundled(slug).map(|b| b.legacy_v1_avatar) else {
        return false;
    };
    if disk_bytes.len() as u64 != expected_len {
        return false;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in disk_bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash == expected_hash
}

/// Returns the exact byte length of the legacy v1 placeholder `AVATAR.png` for fast `stat()` prefiltering.
#[must_use]
pub(super) fn legacy_v1_avatar_len(slug: &str) -> Option<u64> {
    find_bundled(slug).map(|b| b.legacy_v1_avatar.0)
}
