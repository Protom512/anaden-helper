#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    AlreadyOpen,
    Launched,
    Timeout,
    NotReady,
}
