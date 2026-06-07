# Documentation Book Plan

**Date:** 2026-06-07

## Objective

Add a Rust-ecosystem documentation site built with mdBook, modeled on the Rust Book's chapter/search workflow and organized with Django's documentation taxonomy: tutorials, topic guides, how-to guides, and reference.

## Principles

- Keep public docs source in `docs`.
- Keep internal plans under `docs/internal`.
- Keep generated output ignored.
- Prefer one canonical Markdown source where possible.
- Keep Rust API reference in `cargo doc`.
- Keep DocFX available while introducing mdBook, so existing docs links and styling work during transition.

## Information Architecture

- **Tutorials:** guided first steps for new users.
- **Topic guides:** conceptual explanations of architecture, transports, runtime limits, and operations.
- **How-to guides:** task-oriented recipes for production configuration, stress testing, security scanning, and performance comparison.
- **Reference:** protocol, HTTP hardening, API stability, configuration surface, and generated API docs pointers.

## Implementation Tasks

1. Add `book.toml` at the repository root.
2. Add `docs/book-src/SUMMARY.md` and section landing pages.
3. Reuse existing Markdown through mdBook include directives where it avoids duplicated maintenance.
4. Add minimal mdBook CSS for nacelle branding and readable technical pages.
5. Add `scripts/build-book.ps1` to install mdBook if missing, build, serve, and optionally open the book.
6. Ignore generated `docs/book`.
7. Update README with the new book build flow.
8. Build and verify the mdBook site.

## Acceptance

- `mdbook build` succeeds.
- Existing DocFX build still succeeds.
- Existing production validation still succeeds.
- README documents mdBook, DocFX, and `cargo doc` roles clearly.

