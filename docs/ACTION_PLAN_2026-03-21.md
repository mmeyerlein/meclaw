# MeClaw Action Plan — BRAIN.md Gap Analysis & Roadmap

> **Version:** v0.2.0 (2026-03-21)
> **Basis:** BRAIN.md Architecture vs. Implementation Status
> **Benchmark Baseline:** 12% (raw), 22.4% (mit Reader) auf LongMemEval Oracle (500 Fragen)

---

## Completed Phases (v0.1.0 → v0.2.0)

| Phase | Feature | Commit |
|-------|---------|--------|
| ✅ A1 | Temporal Edges — Event→Event chain in AGE | `54cf0d7` |
| ✅ A2 | Entity→Event AGE Links — INVOLVED_IN edges | `2f32d16` |
| ✅ A3 | Graph-Based Retrieval — Cypher traversal in retrieve_bee | `4eec880` |
| ✅ B1 | Trigger Chain — extract→novelty(async)→feedback | `cf8aaa6` |
| ✅ B2 | CTM Retrieval — tick-based embedding drift (optional Stage 4) | `21e56e4` |
| ✅ C1 | Fact-Augmented Keys — facts_text + dual BM25 index | `dfcf013` |
| ✅ C2 | Prototypes — seed, centroid computation, novelty-driven creation | `00758c3` |
| ✅ C3 | User Modeling — preference extraction, auto-create sender entities | `1077d45` |
| ✅ D1 | Evaluator Fix — dual judge (context + answer), no contamination | `d45d63f` |
| ✅ Review | Code review fixes, file renaming, test improvements | `602595b`, `24f6696`, `fc893d7` |

---

## Open Gaps — Prioritized Roadmap

12 features from BRAIN.md that are not yet implemented or incomplete.

| Prio | ID | Feature | Impact | Benchmark-Relevant | Effort |
|------|-----|---------|--------|-------------------|--------|
| 🔴 1 | E1 | **6-Signal Weighted Ranking** | retrieve_bee nutzt RRF+Graph, aber nicht die 6 gewichteten Signale aus BRAIN.md: `similarity*0.25 + reward*0.25 + novelty*0.15 + recency*0.10 + personality_fit*0.15 + graph_distance*0.10` | Direkt — besseres Ranking = bessere Antworten | ~2h |
| 🔴 2 | E2 | **Reward Propagation** (Discounted Returns) | feedback_bee setzt Reward nur auf dem vorherigen Event. BRAIN.md: `reward * POW(0.9, seq_distance)` rückwärts durch die Event-Chain (Tiefe 5) | Direkt — belohnte Events bekommen mehr Gewicht im Ranking | ~1h |
| 🔴 3 | E3 | **Decision Traces + Citation Tracking** | `decision_traces` Tabelle existiert, wird nie befüllt. Kein Code der Citations tracked, keine Authority Curves. BRAIN.md: jede LLM-Entscheidung mit evidence_ids + q_value_estimates loggen | Indirekt — Audit Trail + Stale Precedent Detection | ~3h |
| 🟡 4 | E4 | **ACTIVATES Edges** (Event → Prototype) | BRAIN.md: `(:Event)-[:ACTIVATES {weight}]->(:Prototype)` im AGE Graph. novelty_bee berechnet Prototype-Distance, schreibt aber keine AGE Edge | Graph-Vollständigkeit — ermöglicht Prototype-basierte Traversal | ~1h |
| 🟡 5 | E5 | **ASSOCIATION Edges im AGE Graph** | `prototype_associations` Tabelle existiert mit Hebbian Weights, aber nicht als AGE Edges gespiegelt. BRAIN.md: `(:Prototype)-[:ASSOCIATED {weight}]->(:Prototype)` | Graph-Vollständigkeit — Konzept-Cluster-Traversal | ~1h |
| 🟡 6 | E6 | **Prototype Mitosis** (Conflict Splitting) | consolidation_bee hat `is_flagged_for_review` aber keinen echten Split-Code. BRAIN.md: widersprüchliche Rewards → Prototype splittet in Sub-Konzepte | Datenqualität — verhindert widersprüchliche Prototypes | ~2h |
| 🟡 7 | E7 | **CITATION Edges** (Decision → Event) | BRAIN.md: `(:Decision)-[:CITES {authority, at}]->(:Event)`. Setzt E3 (Decision Traces) voraus | Graph-Vollständigkeit — Authority Curves | ~1h |
| 🟡 8 | E8 | **MemCell Nodes** (Conversation Chunks) | BRAIN.md: `(:MemCell)` als boundary-detected Konversations-Chunks im AGE Graph. Kein Boundary-Detection Code vorhanden | Retrieval-Qualität — ganze Gesprächs-Blöcke statt einzelne Events | ~3h |
| 🟢 9 | E9 | **LLM-Guided Re-Ranking** (Stage 3 Retrieval) | BRAIN.md: LLM liest Cluster-Summaries, entscheidet welche Candidates relevant sind. Teuer aber präzise | Benchmark — könnte +5-10% bringen, aber API-Cost hoch | ~2h |
| 🟢 10 | E10 | **Cross-Agent Retrieval** (Scope Enforcement) | `cross_agent_retrieve` + `share_channel` existieren in Phase 5, aber nicht getestet/verdrahtet. BRAIN.md: Query another agent's memory through shared channels only | Multi-Agent — aktuell nur 1 Agent aktiv | ~2h |
| 🟢 11 | E11 | **Workspace Agent** (Institutional Memory) | Entity-Stub `meclaw:workspace:default` existiert. BRAIN.md: eigenes Brain, Prototypes, Channels, institutional personality | Multi-Agent — Voraussetzung für Team-Memory | ~4h |
| 🟢 12 | E12 | **Stale Precedent Detection** | BRAIN.md: Decisions die lange nicht cited wurden → `is_stale = true`. Citation Authority Curves (trending vs. stale). Setzt E3 + E7 voraus | Langzeit-Gedächtnis-Hygiene | ~1h |

---

## Dependency Graph

```
E1 (6-Signal Ranking) ──────────────────── standalone
E2 (Reward Propagation) ────────────────── standalone
E3 (Decision Traces) ───┬──────────────── standalone
                        ├── E7 (CITATION Edges) depends on E3
                        └── E12 (Stale Precedents) depends on E3 + E7
E4 (ACTIVATES Edges) ───────────────────── standalone
E5 (ASSOCIATION Edges) ─────────────────── standalone
E6 (Prototype Mitosis) ─────────────────── standalone
E8 (MemCell Nodes) ─────────────────────── standalone
E9 (LLM Re-Ranking) ───────────────────── standalone
E10 (Cross-Agent) ──────────────────────── standalone
E11 (Workspace Agent) ──┬──────────────── depends on E10
                        └── benefits from E3
```

---

## Recommended Execution Order

### Wave 1 — Benchmark Impact (E1, E2, E4, E5)
Parallel ausführbar. Direkte Verbesserung des Retrieval-Rankings.

### Wave 2 — Architecture Completeness (E3, E6, E7, E8)
E3 zuerst (Decision Traces), dann E7 (CITATION) und E12 (Stale).

### Wave 3 — Advanced Features (E9, E10, E11, E12)
Nice-to-have. Erst nach Benchmark >40%.

---

## Expected Benchmark Impact

| Kategorie | v0.2.0 Baseline | Nach Wave 1 | Nach Wave 1-2 | Paper Best |
|-----------|----------------|-------------|---------------|------------|
| single-session-assistant | ~46% | 55-65% | 60-70% | ~70% |
| knowledge-update | ~17% | 30-40% | 40-50% | ~60% |
| single-session-user | ~14% | 25-35% | 35-45% | ~60% |
| single-session-preference | ~17% | 25-35% | 30-40% | ~50% |
| multi-session | ~2% | 15-25% | 25-35% | ~50% |
| temporal-reasoning | ~3% | 20-35% | 35-50% | 93.8% (Alinea) |
| **Gesamt** | **~12%** | **~30%** | **~40%** | **60-70%** |

> Temporal >50% erfordert dedizierte temporale Cypher-Queries à la Alinea/Raphtory — deutlich komplexer als die aktuellen Graph-Expansions.

---

*Erstellt: 2026-03-21 | Aktualisiert: 2026-03-21 15:45*
*Basis: BRAIN.md Architecture, v0.2.0 Code Review, LongMemEval Benchmark v1/v2*
