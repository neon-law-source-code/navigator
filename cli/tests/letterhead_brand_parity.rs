//! Grounding test: the firm identity a rendered letter goes out under —
//! `pdf::Letterhead::default()` — must match the identity the website
//! publishes, `views::brand::DEFAULT_BRANDING`.
//!
//! The letterhead is deliberately *not* assembled from brand accessors at
//! render time. `notation render` pins it to source because a rendered letter
//! is a binding artifact, and its identity must not depend on whichever brand
//! bundle happens to be mounted when it is produced. That call is documented
//! where the two are composed, in `cli/src/main.rs`.
//!
//! Pinning to source is not a licence to drift from it. The firm's published
//! voice line moved once already: the website took the new number from
//! `DEFAULT_BRANDING` while the letterhead kept the retired one, so a letter
//! could go out inviting a client to dial a line the firm no longer answers.
//! Nothing failed — the two constants had no relationship to disagree about.
//! This test is that relationship, and it turns the next such change from a
//! reprint into a red build.
//!
//! The comparison is against `DEFAULT_BRANDING`, the source constant, rather
//! than the `views::brand` accessors. The accessors resolve whatever branding
//! is installed in the process, and the letterhead's whole contract is that it
//! answers to source instead — so reading them here would assert the opposite
//! of what the letterhead promises.

use views::brand::DEFAULT_BRANDING;

/// The voice line a client dials off a letter is the one the site publishes.
#[test]
fn letterhead_phone_matches_the_published_brand() {
    assert_eq!(
        pdf::Letterhead::default().phone,
        DEFAULT_BRANDING.firm_phone,
        "the letterhead voice line drifted from views::brand::DEFAULT_BRANDING"
    );
}

/// The inbox a letter invites a reply to is the firm's *published* address —
/// `firm_email`, the one the site advertises — not the `support@` constant the
/// mail pipeline threads on.
#[test]
fn letterhead_email_matches_the_published_brand() {
    assert_eq!(
        pdf::Letterhead::default().email,
        DEFAULT_BRANDING.firm_email,
        "the letterhead inbox drifted from views::brand::DEFAULT_BRANDING"
    );
}

/// The wordmark across the top of a letter is the mark the firm signs its door
/// with, so it tracks `SiteBrand::site_name` rather than the legal entity.
#[test]
fn letterhead_wordmark_matches_the_published_brand() {
    assert_eq!(
        pdf::Letterhead::default().name,
        DEFAULT_BRANDING.firm.site_name,
        "the letterhead wordmark drifted from views::brand::DEFAULT_BRANDING"
    );
}

/// The letterhead prints the website as a reader would type it, so it carries
/// a `www.` prefix the brand constant does not. The host underneath is still
/// `primary_domain`, and that is what may not drift.
#[test]
fn letterhead_web_address_matches_the_published_domain() {
    assert_eq!(
        pdf::Letterhead::default().web,
        format!("www.{}", DEFAULT_BRANDING.primary_domain),
        "the letterhead website drifted from views::brand::DEFAULT_BRANDING"
    );
}
