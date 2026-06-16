# LLM Wiki Schema

This document defines the structure and workflows for the MaaAssistantArknights (MAA) LLM Wiki.

## Structure

- `/docs/llm-wiki/index.md`: Central catalog of all wiki pages.
- `/docs/llm-wiki/log.md`: Chronological log of ingestion and updates.
- `/docs/llm-wiki/sources/`: Summaries of raw source files or modules.
- `/docs/llm-wiki/entities/`: Pages for specific classes, components, or major objects.
- `/docs/llm-wiki/concepts/`: High-level architectural patterns, workflows, and domain concepts.

## Conventions

- Every page must have a link back to `index.md`.
- Use Wikilinks `[[Page Name]]` for internal linking (Obsidian style).
- Maintain a "Source" section in each entity/concept page pointing back to the raw files in `references/MaaAssistantArknights`.

## Workflows

### Ingestion
1. Read a source file or directory from `references/MaaAssistantArknights`.
2. Extract key components, logic, and architectural patterns.
3. Create or update a source page in `/sources/`.
4. Update or create entity pages in `/entities/`.
5. Update or create concept pages in `/concepts/`.
6. Update `index.md` and `log.md`.

### Maintenance
- Periodically check for broken links or outdated summaries.
- Ensure cross-references are dense and useful.
