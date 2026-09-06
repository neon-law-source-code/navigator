//! Session presentation state shared by portal policy and navigation code.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AuthState {
    #[default]
    Anonymous,
    Authenticated,
    ViewingAsDri {
        target_name: String,
        target_email: String,
        csrf_token: String,
    },
    Owner,
    Admin,
    Lawyer,
    Clerk,
}

impl AuthState {
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        !matches!(self, Self::Anonymous)
    }
    #[must_use]
    pub fn is_lawyer_tier(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin | Self::Lawyer)
    }
    #[must_use]
    pub fn is_clerk(&self) -> bool {
        matches!(self, Self::Clerk)
    }
    #[must_use]
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}
