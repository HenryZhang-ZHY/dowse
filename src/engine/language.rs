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
    use super::detect;

    #[test]
    fn detects_by_extension_and_name() {
        assert_eq!(detect("src/main.rs"), Some("Rust"));
        assert_eq!(detect("web/App.TSX"), Some("TSX"));
        assert_eq!(detect("build/Makefile"), Some("Makefile"));
        assert_eq!(detect("LICENSE"), None);
        assert_eq!(detect("archive.unknownext"), None);
    }
}
