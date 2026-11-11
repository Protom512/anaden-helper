use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("Device operation failed: {message}")]
    CommandFailed { message: String },
    #[error("Placement fault: {message}")]
    PlacementFault { message: String },
}
