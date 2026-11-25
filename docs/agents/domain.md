# Domain docs

## Layout and reading rules

This is a single-context repository: domain terms belong in root `GLOSSARY.md`, and architecture decisions belong in `docs/adr/`.

Before exploring domain code, read `GLOSSARY.md` and ADRs relevant to the area being changed, when present. If either is absent, proceed silently. Create domain documents lazily through `/domain-modeling` when terms or decisions are resolved; this setup does not create empty glossary or ADR files.

## Vocabulary

Use glossary terms in issue titles, proposals, hypotheses, and test names. If a needed concept is missing, reconsider whether it belongs to the domain; record genuine terminology gaps for `/domain-modeling`.

## Decisions

Surface conflicts with existing ADRs explicitly, citing the ADR and explaining why it should be reopened rather than silently overriding it.
