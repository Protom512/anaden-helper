use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("Device operation failed: {message}")]
    CommandFailed { message: String },
}
