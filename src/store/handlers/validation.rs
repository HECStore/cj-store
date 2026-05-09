//! Shared input validators for player commands.
//!
//! Pulled out of `player.rs` so the per-command handler modules can share
//! them without cycles.

use crate::constants::MAX_TRANSACTION_QUANTITY;
use crate::types::ItemId;

/// Validate that `item` is a syntactically valid Minecraft item name.
///
/// Accepts ASCII alphanumerics plus `_` and `:` (the `:` allows the optional
/// `minecraft:` namespace prefix that `ItemId::new` strips). On success,
/// returns the canonicalized [`ItemId`] so callers don't need to re-run
/// `ItemId::new`. On error, returns a user-facing message suitable for
/// direct chat reply.
pub(crate) fn validate_item_name(item: &str) -> Result<ItemId, String> {
    if item.is_empty() {
        return Err("Item name cannot be empty. Example: buy cobblestone 64".to_string());
    }

    // Per-character check runs BEFORE `ItemId::new` so the more specific
    // "invalid character '<c>'" message wins over `ItemId::new`'s generic
    // "forbidden character" rejection. Players trying `iron-ingot` see
    // exactly which character is the problem.
    for c in item.chars() {
        if !c.is_ascii_alphanumeric() && c != '_' && c != ':' {
            return Err(format!(
                "Item name contains invalid character '{}'. Use only ASCII letters, numbers, and underscores.",
                c
            ));
        }
    }

    ItemId::new(item).map_err(|_| {
        "Invalid item name. Example items: cobblestone, iron_ingot, diamond".to_string()
    })
}

/// Parse `quantity_str` and enforce `1 <= quantity <= MAX_TRANSACTION_QUANTITY`.
///
/// `operation` is only interpolated into error messages (e.g. "buy", "sell")
/// so examples in the reply match the command the player typed.
pub(crate) fn validate_quantity(quantity_str: &str, operation: &str) -> Result<u32, String> {
    let quantity: u32 = quantity_str.parse().map_err(|_| {
        format!(
            "Invalid quantity '{}'. Please enter a whole number. Example: {} cobblestone 64",
            quantity_str, operation
        )
    })?;

    if quantity == 0 {
        return Err(format!(
            "Quantity must be at least 1. Example: {} cobblestone 64",
            operation
        ));
    }

    if quantity > MAX_TRANSACTION_QUANTITY as u32 {
        return Err(format!(
            "Quantity {} is too large. Maximum is {} items per transaction.",
            quantity, MAX_TRANSACTION_QUANTITY
        ));
    }

    Ok(quantity)
}

/// Validate that `username` matches Minecraft's 3-16 character ASCII
/// alphanumeric (plus underscore) convention.
///
/// The per-rule error messages stay split (length vs charset) because
/// operators see them verbatim and benefit from the precise diagnostic.
/// The terminal `is_valid_username_shape` call is a defense-in-depth
/// belt-and-braces check that the friendly per-rule branches above already
/// cover the full predicate; if the rules ever drift, this gate ensures
/// `validate_username` cannot accept anything the single-source-of-truth
/// predicate rejects.
pub(crate) fn validate_username(username: &str) -> Result<(), String> {
    if username.len() < 3 || username.len() > 16 {
        return Err(format!(
            "Invalid username '{}'. Minecraft usernames are 3-16 characters.",
            username
        ));
    }

    for c in username.chars() {
        if !c.is_ascii_alphanumeric() && c != '_' {
            return Err(format!(
                "Invalid username '{}'. Usernames contain only ASCII letters, numbers, and underscores.",
                username
            ));
        }
    }

    // Defense-in-depth: agree with the single-source-of-truth predicate.
    // Any disagreement here indicates a drift bug; surface it with the
    // generic charset error to match the legacy reject path.
    if !crate::types::user::is_valid_username_shape(username) {
        return Err(format!(
            "Invalid username '{}'. Usernames contain only ASCII letters, numbers, and underscores.",
            username
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- validate_item_name -----------------------------------------------

    #[test]
    fn item_name_accepts_simple_lowercase() {
        assert!(validate_item_name("cobblestone").is_ok());
    }

    #[test]
    fn item_name_accepts_underscore() {
        assert!(validate_item_name("iron_ingot").is_ok());
    }

    #[test]
    fn item_name_accepts_minecraft_prefix() {
        assert!(validate_item_name("minecraft:diamond").is_ok());
    }

    #[test]
    fn item_name_accepts_digits() {
        assert!(validate_item_name("music_disc_11").is_ok());
    }

    #[test]
    fn item_name_rejects_empty() {
        let err = validate_item_name("").unwrap_err();
        assert!(err.contains("cannot be empty"));
    }

    #[test]
    fn item_name_rejects_bare_minecraft_prefix() {
        // `minecraft:` strips to empty, which `ItemId::new` rejects.
        let err = validate_item_name("minecraft:").unwrap_err();
        assert!(err.contains("Invalid item name"));
    }

    #[test]
    fn item_name_rejects_whitespace() {
        let err = validate_item_name("iron ingot").unwrap_err();
        assert!(err.contains("invalid character"));
        assert!(err.contains('\''));
    }

    #[test]
    fn item_name_rejects_hyphen() {
        let err = validate_item_name("iron-ingot").unwrap_err();
        assert!(err.contains("invalid character '-'"));
    }

    #[test]
    fn item_name_rejects_special_characters() {
        for bad in ["iron!", "iron@ingot", "iron$", "iron/ingot", "iron.ingot", "iron,ingot"] {
            assert!(
                validate_item_name(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn item_name_rejects_leading_whitespace() {
        assert!(validate_item_name(" cobblestone").is_err());
    }

    #[test]
    fn item_name_rejects_cyrillic_lookalike() {
        // Second 'o' is Cyrillic U+043E, which `is_alphanumeric` would accept
        // but `is_ascii_alphanumeric` correctly rejects.
        assert!(validate_item_name("diamоnd").is_err());
    }

    // ---- validate_quantity ------------------------------------------------

    #[test]
    fn quantity_accepts_one() {
        assert_eq!(validate_quantity("1", "buy"), Ok(1));
    }

    #[test]
    fn quantity_accepts_typical_value() {
        assert_eq!(validate_quantity("64", "buy"), Ok(64));
    }

    #[test]
    fn quantity_accepts_max() {
        assert_eq!(
            validate_quantity(&MAX_TRANSACTION_QUANTITY.to_string(), "buy"),
            Ok(MAX_TRANSACTION_QUANTITY as u32)
        );
    }

    #[test]
    fn quantity_rejects_zero() {
        let err = validate_quantity("0", "buy").unwrap_err();
        assert!(err.contains("at least 1"));
        assert!(err.contains("buy"));
    }

    #[test]
    fn quantity_rejects_max_plus_one() {
        let over = (MAX_TRANSACTION_QUANTITY as u64 + 1).to_string();
        let err = validate_quantity(&over, "sell").unwrap_err();
        assert!(err.contains("too large"));
        assert!(err.contains(&MAX_TRANSACTION_QUANTITY.to_string()));
    }

    #[test]
    fn quantity_rejects_negative() {
        // u32 parse rejects the leading `-`, so this takes the parse-error branch.
        let err = validate_quantity("-1", "buy").unwrap_err();
        assert!(err.contains("whole number"));
        assert!(err.contains("-1"));
    }

    #[test]
    fn quantity_rejects_non_numeric() {
        let err = validate_quantity("lots", "buy").unwrap_err();
        assert!(err.contains("Invalid quantity 'lots'"));
    }

    #[test]
    fn quantity_rejects_empty() {
        let err = validate_quantity("", "buy").unwrap_err();
        assert!(err.contains("Invalid quantity"));
    }

    #[test]
    fn quantity_rejects_decimal() {
        assert!(validate_quantity("1.5", "buy").is_err());
    }

    #[test]
    fn quantity_rejects_whitespace_padding() {
        assert!(validate_quantity(" 10", "buy").is_err());
        assert!(validate_quantity("10 ", "buy").is_err());
    }

    #[test]
    fn quantity_rejects_u32_overflow() {
        // Larger than u32::MAX — exercises the parse-error branch, not the range check.
        let err = validate_quantity("99999999999999", "buy").unwrap_err();
        assert!(err.contains("Invalid quantity"));
    }

    #[test]
    fn quantity_error_interpolates_operation() {
        let err = validate_quantity("0", "sell").unwrap_err();
        assert!(err.contains("sell"));
    }

    // ---- validate_username ------------------------------------------------

    #[test]
    fn username_accepts_minimum_length() {
        assert!(validate_username("abc").is_ok());
    }

    #[test]
    fn username_accepts_maximum_length() {
        assert!(validate_username("abcdefghijklmnop").is_ok()); // exactly 16
    }

    #[test]
    fn username_accepts_underscores_and_digits() {
        assert!(validate_username("Notch_99").is_ok());
    }

    #[test]
    fn username_rejects_too_short() {
        let err = validate_username("ab").unwrap_err();
        assert!(err.contains("3-16 characters"));
        assert!(err.contains("'ab'"));
    }

    #[test]
    fn username_rejects_empty() {
        assert!(validate_username("").is_err());
    }

    #[test]
    fn username_rejects_too_long() {
        let err = validate_username("abcdefghijklmnopq").unwrap_err(); // 17
        assert!(err.contains("3-16 characters"));
    }

    #[test]
    fn username_rejects_hyphen() {
        let err = validate_username("foo-bar").unwrap_err();
        assert!(err.contains("letters, numbers, and underscores"));
    }

    #[test]
    fn username_rejects_whitespace() {
        assert!(validate_username("foo bar").is_err());
    }

    #[test]
    fn username_rejects_colon() {
        // Colon is allowed for item names but not usernames.
        assert!(validate_username("foo:bar").is_err());
    }

    #[test]
    fn username_rejects_special_characters() {
        for bad in ["foo!", "foo@bar", "foo.bar", "foo$", "foo/bar"] {
            assert!(
                validate_username(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn username_rejects_cyrillic_lookalike() {
        // The 's' is Cyrillic U+0441, which `is_alphanumeric` would accept
        // but `is_ascii_alphanumeric` correctly rejects.
        assert!(validate_username("Notсh_99").is_err());
    }

    #[test]
    fn username_validators_agree_on_edge_corpus() {
        // Pin agreement between the operator-friendly `validate_username`
        // and the single-source-of-truth `is_valid_username_shape`. If a
        // future drift puts them out of sync, this test fails before any
        // user-visible inconsistency reaches production.
        use crate::types::user::is_valid_username_shape;

        let corpus: &[&str] = &[
            // Boundary lengths.
            "ab",                  // 2 — reject
            "abc",                 // 3 — accept
            "abcdefghijklmnop",    // 16 — accept
            "abcdefghijklmnopq",   // 17 — reject
            "",                    // empty — reject
            // Underscore positions.
            "_user_1",             // leading underscore — accept
            "user_1_",             // trailing underscore — accept
            // Digit-only.
            "1234",                // all-digit — accept
            // Hyphen — reject.
            "foo-bar",
            // Single non-ASCII codepoint inside a 3-16-byte string.
            "abç",                 // multi-byte — reject
            // 4-byte / 2-char multi-byte — reject (byte-vs-char asymmetry).
            "éé",
            // Whitespace, colon, dot, NUL.
            "foo bar",
            "foo:bar",
            "foo.bar",
            "foo\0bar",
        ];

        for u in corpus {
            let predicate = is_valid_username_shape(u);
            let validator = validate_username(u).is_ok();
            assert_eq!(
                predicate, validator,
                "drift on {u:?}: is_valid_username_shape={predicate}, validate_username.is_ok={validator}",
            );
        }
    }

    #[test]
    fn username_rejects_multibyte_input() {
        // Direct rejection pin: a 4-byte / 2-char string with byte length
        // in [3,16] must be rejected on the byte-class check.
        let s = "éé";
        assert_eq!(s.len(), 4);
        assert!(validate_username(s).is_err(), "multi-byte must be rejected");
    }
}
