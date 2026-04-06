use std::path::Path;
use std::path::PathBuf;

pub mod model;
pub mod parser_impl;
pub mod proto_path;
pub mod protobuf_abs_path;
pub mod protobuf_ident;
pub mod protobuf_path;
pub mod protobuf_rel_path;

pub fn get_import_file_absoule_path<T: AsRef<str>>(file_path: T) -> Option<String> {
    let relative_path = Path::new(file_path.as_ref());
    if relative_path.is_relative() {
        let current_dir = std::env::current_dir().unwrap();
        let full_path = current_dir.join(relative_path).canonicalize();
        let full_path = match full_path {
            Ok(full_path) => full_path,
            Err(_) => {
                let proto_path_dir =
                    format!("{}/protos", current_dir.to_string_lossy().into_owned());
                let path_buf = PathBuf::from(proto_path_dir);

                let Ok(full_path) = path_buf.join(relative_path).canonicalize() else {
                    return None;
                };
                full_path
            }
        };

        let mut path_buf = PathBuf::new();
        for component in full_path.components() {
            if let Some(str_component) = component.as_os_str().to_str() {
                path_buf.push(str_component);
            }
        }
        let full_path = path_buf.to_string_lossy().into_owned();
        Some(full_path.replace("\\\\?\\", ""))
    } else {
        Some(file_path.as_ref().to_string())
    }
}

pub fn read_file_as_string<T: AsRef<str>>(file_path: T) -> std::io::Result<String> {
    std::fs::read_to_string(file_path.as_ref())
}

pub fn get_file_name_from_path<T: AsRef<str>>(file_path: T) -> Option<String> {
    let path = Path::new(file_path.as_ref());
    // 使用file_name方法获取文件名
    if let Some(file_name) = path.file_name() {
        if let Some(file_name_str) = file_name.to_str() {
            return Some(String::from(file_name_str));
        }
    }
    None
}

pub fn convert_to_snake_case(input: &str) -> String {
    let input_length = input.chars().count();
    let mut result = String::with_capacity(input_length + input_length / 2);
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_uppercase() {
            if !result.is_empty() {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

pub fn get_package_name_from_file_name<T: AsRef<str>>(file_name: T) -> String {
    let package_name = file_name
        .as_ref()
        .replace(".proto", "")
        .replace(".xproto", "");
    let package_name = package_name.replace("import-api-", "");
    let package_name = convert_to_snake_case(&package_name);
    package_name.replace(".", "_").replace("-", "_")
}
