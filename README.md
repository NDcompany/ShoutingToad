# ShoutingToad

An open-source desktop technical website crawler and SEO audit engine built with Rust and egui.

Engineered by [N&D Co.](https://ndcompany.in)

---

## Features

- **Multi-Threaded Website Spider**: Real-time crawling with configurable depth limits, URL filters, robots.txt compliance, and regex extraction.
- **List Mode & Batch Auditing**: Input multiple target URLs simultaneously with automatic formatting and validation.
- **Classic Desktop Grid**: Fast data table with column sorting (status codes, titles, canonicals, directives, word counts, inlinks/outlinks).
- **SERP Snippet Simulator**: Live Google search preview with pixel-width truncation meters for desktop and mobile viewports.
- **Bulk CSV Data Exports**: Export response codes, inlinks, outlinks, missing alt images, and canonical errors.
- **Project Persistence**: Save and reload crawl sessions via portable JSON project files.
- **BYOK AI & Automation Suite**:
  - Title and meta description generator with Google pixel limits.
  - Internal linking discovery engine based on page content analysis.
  - Core Web Vitals layout shift (CLS) and DOM diffing.
  - Executive audit deck generator in responsive HTML/PDF.
  - CI/CD regression workflow generator for GitHub Actions.

---

## Building from Source

### Prerequisites
- Rust stable toolchain (version 1.75 or later)
- Cargo package manager

### Compilation
```bash
cargo build --release
```
The compiled executable will be located in `target/release/`.

### Running Tests
```bash
cargo test
```

---

## Platform Installation

- **Windows**: Run `powershell -ExecutionPolicy Bypass -File installer/install_windows.ps1`
- **Linux**: Run `sudo ./installer/install_linux.sh`
- **macOS**: Run `./installer/package_macos.sh`

---

## Attribution & License

Created by N&D Co. (https://ndcompany.in). Released under the MIT License.
