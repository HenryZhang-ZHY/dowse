//! Display names for the language facet, derived from file names.

/// The language a path is written in, or `None` when the extension is unknown.
pub fn detect(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lowered = name.to_ascii_lowercase();
    if let Some(language) = by_file_name(&lowered) {
        return Some(language);
    }
    let (_, extension) = lowered.rsplit_once('.')?;
    by_extension(extension)
}

/// Whether a file in `language` satisfies `language:<wanted>`: the name
/// ignoring case (`rust`, `c#`, `"visual basic"`), a common alias
/// (`csharp`, `golang`), or an extension of the language (`rs`, `py`, `md`).
pub fn matches(language: Option<&str>, wanted: &str) -> bool {
    let Some(language) = language else {
        return false;
    };
    let wanted = wanted.to_ascii_lowercase();
    let alias = match wanted.as_str() {
        "csharp" => Some("C#"),
        "fsharp" => Some("F#"),
        "golang" => Some("Go"),
        "c++" | "cplusplus" => Some("C++"),
        "shell" | "bash" | "sh" => Some("Shell"),
        "protobuf" => Some("Protocol Buffers"),
        "terraform" => Some("HCL"),
        "vb" | "vbnet" => Some("Visual Basic"),
        _ => None,
    };
    language.eq_ignore_ascii_case(&wanted)
        || alias == Some(language)
        || by_extension(&wanted) == Some(language)
}

fn by_file_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "makefile" | "gnumakefile" => "Makefile",
        "dockerfile" | "containerfile" => "Dockerfile",
        "cmakelists.txt" => "CMake",
        "cargo.lock" => "TOML",
        "justfile" => "Just",
        "rakefile" | "gemfile" => "Ruby",
        _ => return None,
    })
}

fn by_extension(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "rs" => "Rust",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "inl" | "ipp" => "C++",
        "cs" => "C#",
        "fs" | "fsi" | "fsx" => "F#",
        "vb" => "Visual Basic",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "scala" | "sc" => "Scala",
        "groovy" | "gradle" => "Groovy",
        "swift" => "Swift",
        "m" | "mm" => "Objective-C",
        "py" | "pyi" | "pyw" => "Python",
        "rb" | "erb" => "Ruby",
        "php" => "PHP",
        "pl" | "pm" => "Perl",
        "lua" => "Lua",
        "r" => "R",
        "jl" => "Julia",
        "dart" => "Dart",
        "ex" | "exs" => "Elixir",
        "erl" | "hrl" => "Erlang",
        "hs" => "Haskell",
        "ml" | "mli" => "OCaml",
        "clj" | "cljs" | "cljc" | "edn" => "Clojure",
        "zig" => "Zig",
        "nim" => "Nim",
        "v" | "sv" | "svh" => "Verilog",
        "vhd" | "vhdl" => "VHDL",
        "js" | "mjs" | "cjs" => "JavaScript",
        "jsx" => "JSX",
        "ts" | "mts" | "cts" => "TypeScript",
        "tsx" => "TSX",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "astro" => "Astro",
        "html" | "htm" | "xhtml" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "less" => "Less",
        "json" | "jsonc" | "json5" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "xml" | "xsd" | "xsl" | "xslt" | "csproj" | "vbproj" | "fsproj" | "props" | "targets"
        | "resx" | "xaml" | "plist" => "XML",
        "ini" | "cfg" | "conf" => "INI",
        "md" | "markdown" | "mdx" => "Markdown",
        "rst" => "reStructuredText",
        "tex" => "TeX",
        "txt" => "Text",
        "sh" | "bash" | "zsh" | "fish" => "Shell",
        "ps1" | "psm1" | "psd1" => "PowerShell",
        "bat" | "cmd" => "Batch",
        "sql" => "SQL",
        "graphql" | "gql" => "GraphQL",
        "proto" => "Protocol Buffers",
        "cmake" => "CMake",
        "mk" => "Makefile",
        "nix" => "Nix",
        "tf" | "tfvars" | "hcl" => "HCL",
        "dockerfile" => "Dockerfile",
        "sln" => "Visual Studio Solution",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{detect, matches};

    #[test]
    fn detects_by_extension_and_name() {
        assert_eq!(detect("src/main.rs"), Some("Rust"));
        assert_eq!(detect("web/App.TSX"), Some("TSX"));
        assert_eq!(detect("build/Makefile"), Some("Makefile"));
        assert_eq!(detect("LICENSE"), None);
        assert_eq!(detect("archive.unknownext"), None);
    }

    #[test]
    fn matches_names_aliases_and_extensions() {
        assert!(matches(Some("Rust"), "rust"));
        assert!(matches(Some("Rust"), "rs"));
        assert!(matches(Some("C#"), "csharp"));
        assert!(matches(Some("C#"), "C#"));
        assert!(matches(Some("Visual Basic"), "visual basic"));
        assert!(matches(Some("TypeScript"), "ts"));
        assert!(!matches(Some("TSX"), "ts"));
        assert!(!matches(Some("Rust"), "go"));
        assert!(!matches(None, "rust"));
    }
}
