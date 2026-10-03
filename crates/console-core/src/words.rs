//! What the console calls things, where that differs from the API (#70).
//!
//! The platform's word for a sealed, immutable image on forge is
//! "golden", and that word stays in every API, id, kind and field so
//! automation keeps working. A person reads "registry image": nothing
//! runs *on* one — a pod, a VM, a boot runs on a copy-on-write clone of
//! it, its instance — and calling the registry entry by the clone's
//! job was the confusion. This is the one table that turns the API's
//! token into the page's word, mirrored by `web/src/lib/ui/words.js`
//! for the tokens the SPA shows itself (kinds, relation and metric
//! names).

/// The page's word for an API token, or the token itself.
pub fn term(token: &str) -> &str {
    match token {
        "golden" => "registry image",
        "goldens" => "registry images",
        "slab_golden" => "slab registry image",
        "slab_goldens" => "slab registry images",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_token_is_translated() {
        assert_eq!(term("golden"), "registry image");
        assert_eq!(term("goldens"), "registry images");
        assert_eq!(term("slab_golden"), "slab registry image");
        // A name is data, not a word to translate.
        assert_eq!(term("golden-stormlb"), "golden-stormlb");
        assert_eq!(term("clone"), "clone");
    }
}
