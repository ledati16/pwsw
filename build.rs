use std::env;
use std::fs;
use std::path::Path;

fn main() {
    built::write_built_file().expect("Failed to acquire build-time information");

    // Generate man pages
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let man_out_dir = Path::new(&out_dir).join("man");
    fs::create_dir_all(&man_out_dir).unwrap();

    let man_sources = [("man/pwsw.1.md", "pwsw", 1), ("man/pwsw.5.md", "pwsw", 5)];

    for (src_path, title, section) in man_sources {
        println!("cargo:rerun-if-changed={src_path}");

        let markdown =
            fs::read_to_string(src_path).expect("Failed to read man page markdown source");

        // mandown 1.1.0: convert(markup, title, section) -> String
        let roff = mandown::convert(&markdown, title, section);

        let dst_filename = format!("{title}.{section}");
        fs::write(man_out_dir.join(dst_filename), roff)
            .expect("Failed to write generated man page");
    }
}
