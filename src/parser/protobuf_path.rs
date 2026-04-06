use std::fmt;

use super::protobuf_abs_path::ProtobufAbsPath;
use super::protobuf_rel_path::ProtobufRelPath;

/// Protobuf identifier can be absolute or relative.
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub enum ProtobufPath {
    Abs(ProtobufAbsPath),
    Rel(ProtobufRelPath),
}

impl ProtobufPath {
    pub fn new<S: Into<String>>(path: S) -> ProtobufPath {
        let path = path.into();
        if path.starts_with('.') {
            ProtobufPath::Abs(ProtobufAbsPath::new(path))
        } else {
            ProtobufPath::Rel(ProtobufRelPath::new(path))
        }
    }

    pub fn get_path(&self) -> String {
        match self {
            ProtobufPath::Abs(path) => path.path.clone(),
            ProtobufPath::Rel(path) => path.path.clone(),
        }
    }

    pub fn _resolve(&self, package: &ProtobufAbsPath) -> ProtobufAbsPath {
        match self {
            ProtobufPath::Abs(p) => p.clone(),
            ProtobufPath::Rel(p) => {
                let mut package = package.clone();
                package.push_relative(p);
                package
            }
        }
    }
}

impl fmt::Display for ProtobufPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtobufPath::Abs(p) => write!(f, "{}", p),
            ProtobufPath::Rel(p) => write!(f, "{}", p),
        }
    }
}
