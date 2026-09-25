//! How `/app` pages name a brand's public website.
//!
//! A compiled house-brand key is scoped to one production host — the `www`
//! form. A runtime-only `brand` row has no compiled
//! host.
//!
//! This table is the `/app` listing contract and must stay in step with
//! [`views::brand::BrandKey::hosts`] and [`views::brand::BrandKey::LIVE`].
//! It lives here as string data so the wasm client can render listings
//! without taking a `views` dependency.

/// One compiled house brand's production website.
struct CompiledSite {
    key: &'static str,
    www: &'static str,
    live: bool,
}

const COMPILED_SITES: &[CompiledSite] = &[
    CompiledSite {
        key: "neon",
        www: "www.neonlaw.com",
        live: true,
    },
    CompiledSite {
        key: "delete-your-data",
        www: "www.deleteyourdata.com",
        live: true,
    },
    CompiledSite {
        key: "vesta",
        www: "www.vestaestateplanning.com",
        live: true,
    },
    CompiledSite {
        key: "misericordia",
        www: "www.misericordialaw.com",
        live: true,
    },
    CompiledSite {
        key: "abhaya",
        www: "www.abhayaimmigration.com",
        live: true,
    },
    CompiledSite {
        key: "delete-your-debt",
        www: "www.deleteyourdebt.com",
        live: true,
    },
    CompiledSite {
        key: "lawyer-shook",
        www: "www.lawyershook.com",
        live: true,
    },
    CompiledSite {
        key: "summons",
        www: "www.summonsdefense.nyc",
        live: true,
    },
    CompiledSite {
        key: "daybridge",
        www: "www.daybridgedivorce.com",
        live: true,
    },
    CompiledSite {
        key: "death-and-divorce",
        www: "www.deathanddivorcelaw.com",
        live: true,
    },
];

/// One brand key as `/app` listings should print it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandWebsite {
    pub key: String,
    pub production_host: Option<&'static str>,
    pub live: bool,
}

impl BrandWebsite {
    #[must_use]
    pub fn from_key(key: impl AsRef<str>) -> Self {
        let key = key.as_ref().to_string();
        match COMPILED_SITES.iter().find(|site| site.key == key) {
            Some(site) => Self {
                production_host: Some(site.www),
                live: site.live,
                key,
            },
            None => Self {
                key,
                production_host: None,
                live: false,
            },
        }
    }

    /// Key plus production host, for firm and project listings.
    #[must_use]
    pub fn attached_line(&self) -> String {
        match self.production_host {
            Some(host) => format!("{} ({host})", self.key),
            None => format!("{} (no public host)", self.key),
        }
    }

    /// Host only, for the brands home and edit pages.
    #[must_use]
    pub fn host_line(&self) -> String {
        match self.production_host {
            Some(host) => host.to_string(),
            None => "No public host".to_string(),
        }
    }
}

/// Join attached lines for a Firm's brand keys.
#[must_use]
pub fn attached_lines(keys: &[String]) -> String {
    if keys.is_empty() {
        "No house brands attached.".to_string()
    } else {
        keys.iter()
            .map(|key| BrandWebsite::from_key(key).attached_line())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::{attached_lines, BrandWebsite, COMPILED_SITES};

    #[test]
    fn a_live_compiled_key_names_its_www_host() {
        let site = BrandWebsite::from_key("delete-your-data");
        assert_eq!(site.production_host, Some("www.deleteyourdata.com"));
        assert!(site.live);
        assert_eq!(
            site.attached_line(),
            "delete-your-data (www.deleteyourdata.com)"
        );
        assert_eq!(site.host_line(), "www.deleteyourdata.com");
    }

    #[test]
    fn summons_lists_its_public_holding_page() {
        let site = BrandWebsite::from_key("summons");
        assert_eq!(site.production_host, Some("www.summonsdefense.nyc"));
        assert!(site.live);
        assert_eq!(site.attached_line(), "summons (www.summonsdefense.nyc)");
        assert_eq!(site.host_line(), "www.summonsdefense.nyc");
    }

    #[test]
    fn a_runtime_key_has_no_public_host() {
        let site = BrandWebsite::from_key("acme-brand");
        assert!(site.production_host.is_none());
        assert!(!site.live);
        assert_eq!(site.attached_line(), "acme-brand (no public host)");
        assert_eq!(site.host_line(), "No public host");
    }

    #[test]
    fn every_compiled_listing_host_is_the_www_form() {
        for site in COMPILED_SITES {
            assert!(
                site.www.starts_with("www."),
                "{} production host is the www form: {}",
                site.key,
                site.www
            );
        }
        let live: Vec<&str> = COMPILED_SITES
            .iter()
            .filter(|site| site.live)
            .map(|site| site.key)
            .collect();
        assert_eq!(
            live,
            [
                "neon",
                "delete-your-data",
                "vesta",
                "misericordia",
                "abhaya",
                "delete-your-debt",
                "lawyer-shook",
                "summons",
                "daybridge",
                "death-and-divorce",
            ],
            "the live listing set is the launched house brands"
        );
    }

    #[test]
    fn attached_lines_join_keys_and_name_an_empty_firm() {
        assert_eq!(attached_lines(&[]), "No house brands attached.");
        assert_eq!(
            attached_lines(&["neon".to_string(), "summons".to_string()]),
            "neon (www.neonlaw.com), summons (www.summonsdefense.nyc)"
        );
    }
}

#[cfg(all(test, feature = "server"))]
mod registry_parity {
    use super::BrandWebsite;
    use views::brand::BrandKey;

    /// The `/app` listing table is a copy of the compiled registry. If a key,
    /// `www` host, or live flag drifts, the inventory lies.
    #[test]
    fn the_listing_table_matches_the_compiled_registry() {
        assert_eq!(super::COMPILED_SITES.len(), BrandKey::ALL.len());
        for key in BrandKey::ALL {
            let site = BrandWebsite::from_key(key.as_str());
            assert_eq!(
                site.production_host,
                Some(key.canonical_host()),
                "{} www host",
                key.as_str()
            );
            assert_eq!(site.live, key.is_live(), "{} live flag", key.as_str());
        }
    }
}
