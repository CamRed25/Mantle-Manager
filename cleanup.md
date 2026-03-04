# Cleanup Log

Tracks code that has been retired from the active source tree and moved to `doa/`. Each entry records what was removed, why, and where it lives now. Required by RULE_OF_LAW §4.1.

---

## Format

```
### YYYY-MM-DD — <short title>
- **Removed:** `path/to/file.rs` (or module name)
- **Reason:** Why it was retired
- **DOA path:** `doa/YYYY-MM-DD_<name>/`
- **Notes:** Any useful context for future readers
```

---

## Log

### 2026-03-04 — Initial repo audit archived
- **Removed:** `audit.md` (project document)
- **Reason:** One-time audit deliverable. All 14 findings were reviewed and actioned. No ongoing value at root.
- **DOA path:** `doa/2026-03-04_audit.md`
- **Notes:** `cleanup.md` and `conflict.md` remain at root — they are permanent governance files required by RULE_OF_LAW §4.1 and §2.
