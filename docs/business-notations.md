# Business notations

The public library contains one Neon Law review draft for each of the twelve templates in [General Legal’s CC0
repository](https://github.com/General-Legal/legal-templates). The adaptation uses shorter sentences, explicit
commercial schedules, typed parties and dates, and a required lawyer-review step. It does not represent an attorney’s
approval of a particular transaction.

## Sources

Source revision: `6d6805425eabd41bed86fc1e2ec51612760f716c`. Retrieved September 24, 2026. The SHA-256 values identify
the original `template.md` bytes used for comparison. General Legal’s source material remains CC0; original Navigator
prose, questionnaires, and workflows follow the repository’s BUSL-1.1 terms. No claim is made to exclusive rights in the
CC0 material.

- Mutual Non-Disclosure Agreement (`templates/mutual-nda/template.md`)
  SHA-256: `2b790eda57208b5b760e0d61bc5f5a074eaf21c41f6f5473d4cd8ba9015707a8`
- One-Way Non-Disclosure Agreement (`templates/one-way-nda/template.md`)
  SHA-256: `639846700c0cda7add7d7e128d6bcd2486d5a8f8e05e5ccad45b13842d184237`
- Advisor Agreement (`templates/advisor-agreement/template.md`)
  SHA-256: `bd6851ce93e61bb8f9256a2fe9a27569a009944996778056c52ee0faec59e8b1`
- Master Services Agreement (`templates/master-services-agreement/template.md`)
  SHA-256: `c30f258e5378f9f6855fc37ac66607b2a2aa6f71fad7d4f737b1a4781a3f5c0d`
- Employee Offer Letter (California Exempt) (`templates/employee-offer-letter/template.md`)
  SHA-256: `d6245f09c91c1626b276d763eea8343c5ee80eeedbdc242760dc1af69a7e2ec2`
- Business Associate Agreement (`templates/business-associate-agreement/template.md`)
  SHA-256: `1f9902a8f60ff5c40c184e66e9625e14f6b8eae7987054dae6a1d4e3f586746a`
- Data Processing Addendum (U.S.) (`templates/dpa-us/template.md`)
  SHA-256: `1964d2af87d6af379ae56f3395a0ffc09f513f809e63938ee5abff88889a87a8`
- Data Processing Addendum (Global) (`templates/dpa-global/template.md`)
  SHA-256: `27e8c6ea6189900ce275e7b6a7a1d46a50556137b171776f734882a5507f88e5`
- Cookie Notice (`templates/cookie-notice/template.md`)
  SHA-256: `f95930b862b2ca87cbbaf36710a35322f5ba67555371e2f2321ff3dfb3d2006a`
- Privacy Policy (U.S.) (`templates/privacy-policy-us/template.md`)
  SHA-256: `9e5dd940135752e69a6fda3254ce199013e15fcb29efc38ad5cd816d5994000c`
- Privacy Policy (GDPR Enhanced) (`templates/privacy-policy-gdpr/template.md`)
  SHA-256: `d51ebccc012a5be6a2858dd65e018fd50756d21263bfce8e0497c1d54ef96657`
- Terms of Use (`templates/terms-of-use/template.md`)
  SHA-256: `b36712ca8c15160ad39530f18c9feaa0bcba007254ca498109796a92f87a076c`

## Deliberate drafting choices

**NDAs.** Shorter confidentiality provisions; negotiated duration and courts; protected reports preserved; no automatic
feedback license or bond waiver.

**Advisors.** Compensation, vesting, and prior materials require completion; equity requires separate approval;
publicity requires consent; no assumed exclusivity.

**Employment.** California classification requires review of actual duties and current thresholds. Arbitration is
omitted. Invention assignment and required notices remain separate documents.

**MSA.** Generic services and orders replace product-specific AI infrastructure. Service levels, liability, and exit
terms require completion. Courts replace arbitration. Model training requires separate authorization.

**HIPAA.** Explicit safeguards, incident reporting, individual-rights assistance, subcontractor duties, HHS access, and
return or destruction. Confirm any additional health-data laws.

**DPAs.** Actual processing, security, and subprocessor schedules must be completed. The global version requires
executed transfer instruments and annexes; its summary is not a substitute for the SCCs or UK instrument.

**Privacy and cookies.** Actual practices must be supplied and tested. No vendor, tracking consent, opt-out
implementation, or age policy is assumed.

**Website terms.** Assent, fees, renewals, content rights, local notices, and liability require completion. No
arbitration, class waiver, or broad third-party release is imposed.

The drafting review uses the Legal Council’s accountability and enforceability lenses. The Client Council’s access and
clarity lenses favor short sentences and named choices. These review lenses do not replace a licensed lawyer’s approval.

## Runtime contract

The public catalog and preview list share `BUSINESS_NOTATIONS` in `neon/src/firm_pages.rs`. The canonical seed installs
the twelve templates. Each paired workflow spec must match its template frontmatter. Existing composition tests check
that every seeded code has a walkable questionnaire and that each public template has one card and one preview.

Parties use `entity` or `person` states, dates use `custom_datetime`, notice contacts use `people`, and governing law
uses `jurisdiction`. The `business_*` text roles are negotiated provisions or factual schedules, explicitly listed in
rule N117. They are lawyer-facing. They do not create a second store of client identities.

## Home-page claims

The user supplied the fee model, Bay Area roots, access-to-justice and technology-community activity, Justice Tech
Association membership, Rust NYC connection, independent funding, and pro bono commitment. No comparative savings
percentage or pro bono allocation is asserted. The Rust claim is limited to memory-safety benefits and the repository’s
actual checks and review workflow. The user supplied the production-engineering statement as firm positioning. Existing
trust-deposit and page-limit disclosures remain pending any separate change to engagement terms.

The user also confirmed 12 years of experience, nine figures in exits, and coverage of the agreements listed by General
Legal. The home page uses those aggregate facts without identifying clients, assigning a precise exit value, or
promising a result. The service directory describes the firm's work beyond the twelve public template drafts.

Ferris is Karen Rustad Tölva’s public-domain artwork from [Rustacean.net](https://rustacean.net/), served locally as
SVG.
