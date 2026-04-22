# DR Rehearsal History

> **Append-only log.** Never edit or delete past entries. Add new entries at
> the bottom of the file. Follow the template in
> [`20-dr-rehearsal.md` §4](20-dr-rehearsal.md#4-post-mortem-template).

---

## Template (copy for each new drill)

```markdown
## Drill: <Quarter> <Year> — <Scenario name>

- **Date:** YYYY-MM-DD
- **Operator:** <name>
- **Scenario:** Q<N> — <one-line description>
- **Start time (UTC):** HH:MM
- **End time (UTC):** HH:MM
- **Total elapsed:** Xh Ym
- **Outcome:** PASS | PARTIAL | FAIL

### Pre-conditions met?

- [ ] <pre-condition 1>
- [ ] <pre-condition 2>

### Action sequence notes

| Step | Expected | Actual | OK? |
|------|----------|--------|-----|
| 1 | ... | ... | ✓ / ✗ |

### Success criteria results

- [ ] Criterion 1 — result
- [ ] Criterion 2 — result

### What went well

- 

### What didn't go well

- 

### Runbook fixes needed before next quarter

- [ ] Fix 1 (owner: <name>, due: YYYY-MM-DD)

### Actual RTO / RPO (if applicable)

- **RTO:** Xh Ym
- **RPO:** Xh
```

---

<!-- Drill entries go below this line, newest at the bottom -->

<!-- EXAMPLE (remove before first real entry):
## Drill: Q1 2026 — Full server loss

- **Date:** 2026-01-13
- **Operator:** Adam
- **Scenario:** Q1 — Full server loss (restore from Postgres dump + blob rsync)
- **Start time (UTC):** 09:00
- **End time (UTC):** 10:47
- **Total elapsed:** 1h 47m
- **Outcome:** PASS

### Pre-conditions met?

- [x] Previous-night backup set exists and accessible
- [x] Replacement host available

### Action sequence notes

...

### Success criteria results

- [x] GET /healthz 200 ✓
- [x] GET /readyz 200 ✓
- [x] Session count matches pre-drill snapshot ✓
- [x] At least one agent bundle ingested after restore ✓
- [x] Total elapsed ≤ 2 h ✓

### What went well

- Backup restore was faster than the 4 h RTO target.

### What didn't go well

- Blob rsync took longer than expected due to directory size.

### Runbook fixes needed before next quarter

- [ ] Add blob size check to pre-conditions (owner: Adam, due: 2026-02-01)

### Actual RTO / RPO (if applicable)

- **RTO:** 1h 47m
- **RPO:** 22h (last dump was ~22 h before drill start)
-->
