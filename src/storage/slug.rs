//! Command id generation from a display name.
//!
//! ## Public surface
//!
//! - [`generate_slug`]: turn an action name into a collision-free, kebab-case
//!   ASCII id suitable for [`crate::storage::models::Command::id`].
//!
//! Also exposes [`fold_ascii_lower`] (crate-private) — a manual ASCII fold of
//! common French diacritics reused by
//! [`crate::storage::categories`]'s reserved-name guard so both agree on what
//! counts as "the same" letter when comparing case/accent-insensitively.
//!
//! ## Deferred id-generation seam (preferences config editor, part 1)
//!
//! [`generate_slug`] is the callable API seam for the future action editor
//! (Part 3). It is fully tested but not yet wired to the UI.

// Staged slug-generation API (preferences config editor, part 1): fully
// tested but not yet wired to the UI (Part 3).
#![allow(dead_code)]

/// Fold a string to lowercase ASCII, mapping common French diacritics to
/// their unaccented base letter. Characters outside the fold table are
/// passed through unchanged (including non-ASCII ones); callers that need a
/// strict `[a-z0-9-]` result (like [`generate_slug`]) must filter further.
///
/// This is shared between [`generate_slug`] and
/// [`crate::storage::categories`]'s reserved-name guard so accent/case
/// folding behaves identically in both places.
pub(crate) fn fold_ascii_lower(input: &str) -> String {
    // Expand the two French ligatures to their two-letter equivalent before
    // the per-character fold below (they have no single-char ASCII base).
    let normalized = input
        .replace('œ', "oe")
        .replace('Œ', "OE")
        .replace('æ', "ae")
        .replace('Æ', "AE");

    let mut out = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        for lower in ch.to_lowercase() {
            let folded = match lower {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
                'ç' => 'c',
                'è' | 'é' | 'ê' | 'ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' => 'i',
                'ñ' => 'n',
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
                'ù' | 'ú' | 'û' | 'ü' => 'u',
                'ý' | 'ÿ' => 'y',
                other => other,
            };
            out.push(folded);
        }
    }
    out
}

/// Fallback slug used when accent-folding + normalization yields an empty
/// string (e.g. a name made entirely of punctuation/emoji).
const FALLBACK_SLUG: &str = "commande";

/// Upper bound on the numeric anti-collision suffix search, per the plan's
/// risk register (pathological input with many ids sharing a prefix must not
/// loop unboundedly).
const MAX_SUFFIX_ATTEMPTS: u32 = 1000;

/// Turn `name` into a collision-free, kebab-case ASCII id.
///
/// Steps:
/// 1. Accent-fold + lowercase via [`fold_ascii_lower`].
/// 2. Collapse every run of non `[a-z0-9]` bytes into a single `-`, trimming
///    leading/trailing dashes.
/// 3. If the result is empty, fall back to `"commande"`.
/// 4. If the result collides with an id in `existing_ids`, append a numeric
///    suffix (`-2`, `-3`, ...) up to [`MAX_SUFFIX_ATTEMPTS`]; past that bound,
///    fall back to a deterministic tail derived from the number of existing
///    ids sharing the base slug so the function still terminates and still
///    never returns empty.
///
/// Never returns an empty string.
pub fn generate_slug(name: &str, existing_ids: &[String]) -> String {
    let folded = fold_ascii_lower(name);

    let mut slug = String::with_capacity(folded.len());
    let mut last_was_dash = false;
    for c in folded.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        slug = FALLBACK_SLUG.to_string();
    }

    if !existing_ids.iter().any(|id| id == &slug) {
        return slug;
    }

    for n in 2..=MAX_SUFFIX_ATTEMPTS {
        let candidate = format!("{slug}-{n}");
        if !existing_ids.iter().any(|id| id == &candidate) {
            return candidate;
        }
    }

    // Bounded search exhausted: deterministic tail derived from how many
    // existing ids share the base slug prefix, so this still terminates and
    // still (almost certainly) avoids a collision without looping forever.
    let sharing_prefix = existing_ids
        .iter()
        .filter(|id| id.starts_with(slug.as_str()))
        .count();
    format!("{slug}-{}", MAX_SUFFIX_ATTEMPTS as usize + sharing_prefix)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // fold_ascii_lower
    // -----------------------------------------------------------------------

    #[test]
    fn folds_common_french_diacritics() {
        assert_eq!(fold_ascii_lower("Éditeur Étendu"), "editeur etendu");
        assert_eq!(fold_ascii_lower("Sans catégorie"), "sans categorie");
        assert_eq!(fold_ascii_lower("Cœur"), "coeur");
        assert_eq!(fold_ascii_lower("naïve"), "naive");
    }

    // -----------------------------------------------------------------------
    // generate_slug — accent folding + kebab-case
    // -----------------------------------------------------------------------

    #[test]
    fn accented_french_name_produces_ascii_slug() {
        let slug = generate_slug("Éditeur Étendu", &[]);
        assert_eq!(slug, "editeur-etendu");
        assert!(slug.is_ascii());
    }

    #[test]
    fn punctuation_is_collapsed_to_single_dashes() {
        let slug = generate_slug("Ping !! & Trace-route", &[]);
        assert_eq!(slug, "ping-trace-route");
    }

    // -----------------------------------------------------------------------
    // generate_slug — collision handling
    // -----------------------------------------------------------------------

    #[test]
    fn colliding_name_gets_deterministic_numeric_suffix() {
        let existing = vec!["editeur-etendu".to_string()];
        let slug = generate_slug("Editeur Etendu", &existing);
        assert_eq!(slug, "editeur-etendu-2");
    }

    #[test]
    fn suffix_search_finds_first_free_slot() {
        let existing = vec![
            "notepad".to_string(),
            "notepad-2".to_string(),
            "notepad-3".to_string(),
        ];
        let slug = generate_slug("Notepad", &existing);
        assert_eq!(slug, "notepad-4");
    }

    #[test]
    fn generate_slug_is_deterministic() {
        let existing = vec!["notepad".to_string()];
        let first = generate_slug("Notepad", &existing);
        let second = generate_slug("Notepad", &existing);
        assert_eq!(first, second);
    }

    // -----------------------------------------------------------------------
    // generate_slug — never empty
    // -----------------------------------------------------------------------

    #[test]
    fn empty_name_falls_back_to_commande() {
        assert_eq!(generate_slug("", &[]), "commande");
    }

    #[test]
    fn punctuation_only_name_falls_back_to_commande() {
        assert_eq!(generate_slug("!!! ---", &[]), "commande");
    }

    #[test]
    fn fallback_slug_also_gets_anti_collision_suffix() {
        let existing = vec!["commande".to_string()];
        let slug = generate_slug("###", &existing);
        assert_eq!(slug, "commande-2");
    }

    #[test]
    fn result_is_never_empty_across_varied_inputs() {
        for name in ["", "   ", "---", "😀😀😀", "Réseau Étendu", "a"] {
            let slug = generate_slug(name, &[]);
            assert!(!slug.is_empty(), "slug for {name:?} must not be empty");
        }
    }
}
