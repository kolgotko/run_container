extern crate path_absolutize;

pub use std::path::{ Path, PathBuf, MAIN_SEPARATOR };
pub use path_absolutize::*;


#[macro_export]
macro_rules! path_join {
    ($prefix:expr, $($child:expr),+) => {
        {
            let mut path = PathBuf::from($prefix);
            let separator = MAIN_SEPARATOR.to_string();
            $(
                let child = Path::new($child);
                let child = child.strip_prefix(&separator).unwrap_or(child);
                path.push(child); 
            )*
            path
        }
    }
}

#[macro_export]
macro_rules! path_resolve {
    ($path:expr) => {
        {
            let path = path_join!(MAIN_SEPARATOR.to_string(), $path);
            PathBuf::from(path)
                .absolutize()
        }
    };
    ($prefix:expr, $path:expr) => {
        {
            let path = path_join!($prefix, $path);
            println!("{:?}", path);
            PathBuf::from(path)
                .absolutize()
        }
    };
}
