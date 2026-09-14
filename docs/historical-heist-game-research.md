# Historical-heist inspiration for a separate flagship Activity Pack

Researched **2026-09-11**. This is decision support, not an adopted design, implementation plan, or change to Agent Heist. Historical claims come from first-party museums, archives, or law enforcement. The proposed game is a fictional recovery investigation, not a real-crime reenactment.

## Comparison

| Inspiration | Verified historical facts | Useful design material | Adaptation tradeoffs |
| --- | --- | --- | --- |
| **Isabella Stewart Gardner Museum theft (1990)** | Thirteen works were stolen; the case remains unsolved; the museum and FBI still seek their return. Empty frames remain as placeholders. [Museum theft page](https://www.gardnermuseum.org/about/theft), [the frames](https://www.gardnermuseum.org/blog/five-frames-left-behind), [FBI history](https://www.fbi.gov/history/cases-and-criminals/isabella-stewart-gardner-museum-heist) | Visible absence; evidence gaps; competing interpretations; recovery rather than acquisition. | **Recommended starting inspiration.** Invent the case and solution so the game does not imply a finding about the ongoing investigation. |
| **Great Train Robbery (1963)** | A Glasgow-to-London mail train was stopped and more than £2.63 million stolen. Official histories emphasize the later investigation and forensic evidence, including fingerprints on ordinary objects. The National Archives identifies the underlying inspectorate file as HO 287/1496. [National Archives event and catalogue reference](https://www.nationalarchives.gov.uk/whats-on/events/the-great-train-robbery-a-forensic-journey/), [British Transport Police history](https://www.btp.police.uk/police-forces/british-transport-police/areas/campaigns/our-history/crime-history/great-train-robbery/) | A moving deadline, distributed evidence, a plan disrupted by human limitations, and mundane objects becoming decisive clues. | Stronger action-heist atmosphere, but likely needs more movement, scheduling and visual simulation. Keep fictional game mechanics separate from claims of historical accuracy. |
| **Brink's robbery (1950)** | The FBI records a six-year investigation with physical evidence, dead ends, interviews, financial tracing, and corroboration before convictions. [FBI historical case file](https://www.fbi.gov/history/cases-and-criminals/brinks-robbery) | Evidence convergence, noisy tips, and provable case versus plausible story. | The long investigation does not directly supply a short-session structure; compressing it needs clearly fictional events and characters. |

**Recommendation — design inference:** use the Gardner theft's unresolved recovery, missing-object silhouettes, and evidentiary ambiguity as the emotional seed. Add the train case's deadline and the Brink's case's requirement to corroborate claims. None of the sources validates the proposed game, agent experience, or commercial appeal.

## Recommended fictional mission: *The Empty Gallery*

**Fictional premise.** Five cultural objects vanished from Veyra's Bellwether Gallery in 1978. Before an estate collection is dispersed, photographs suggest one is present under false provenance. The default mission is a cooperative 12-minute Activity Phase: one human in the **Crew Lead Role** and two House or BYO specialist Agent Participants. The Activity Pack owns evidence, legal Actions, clock, and Outcomes. A two-team, 25-minute competitive version is a later variant.

This remains a heist fantasy through a hard clock, hidden object, aliases, scarce access, competitive pressure, and risky commitment. The fantasy is **taking the truth back from people concealing it**, not entering a real facility.

### Evidence loop, not document dumping

The authored case is a small graph of claims and witnesses. A valid recovery petition needs three checkable links: **object identity**, **custody discontinuity**, and **current lot correspondence**. Each has two possible routes, a near-match, and a contradiction. Tool results are short records with source IDs; the client visualizes but does not decide truth.

Example chain: an agent searches a fictional accession catalogue, extracts a maker's repair notation, requests a lab comparison against a current photograph, then checks whether a shipping alias appears in an estate ledger. The Pack deterministically verifies whether the submitted evidence IDs support the claimed links. Free-form agent prose never establishes success.

Scarce time and access create tradeoffs: a conservation test consumes a shared lab slot; an interview may reveal motive but not admissible provenance; disclosing a clue can unlock a reluctant witness. Failed searches cost time, so exhaustive querying is not optimal.

### What the two agents actually do

**Archivist agent (House or BYO).** It receives only its authorized Projection and current Action Offers, then uses bounded game tools such as `search_catalogue`, `compare_marks`, `cross_reference_ledger`, and `propose_evidence_chain`.

- If a catalogue match is exact, it tests the repair notation against the photograph.
- If the mark is ambiguous, it branches to a conservator note or asks the Crew Lead to spend the lab slot.
- If a ledger alias conflicts with the date, it flags the contradiction rather than smoothing it away and proposes one next check.

**Field agent (House or BYO).** It uses `request_interview`, `check_statement`, `offer_exchange`, and `recommend_recovery_claim`.

- If a witness statement conflicts with the manifest, it can re-interview using one cited discrepancy.
- If access is denied, it can offer one discovered clue to a witness for a bounded, recorded exchange.
- It may recommend an early claim on a weaker chain or further investigation that risks the deadline.

These are branching, state-changing choices: the useful next call depends on prior results, time, disclosures, and Crew Lead resource decisions. House and BYO agents face equivalent Actions and evidence rules for equivalent Roles and knowledge; exact Action Offers remain Membership- and view-bound. Model narration is flavor, not authority.

### Crew Lead leverage and the opposing choice

The **Crew Lead Role** assigns objectives, spends the lab slot, accepts exchanges, and commits the final claim. The first test seats a human as Crew Lead, conversing with both agents. These are Role authorities, not human-kind privileges: a later configuration may seat an Agent Participant as captain. Specialist agents are first-class peers whose accepted Actions change Activity State; their Roles cannot commit Crew Lead Actions.

Late evidence reveals that an immediate private claim would recover the object, while public disclosure of the full ledger would delay recovery but expose a wider network. **Recover now** scores the object but may leave the network intact; **publish the ledger** can produce a public-interest Outcome but risks losing recovery before the deadline. Neither is the disguised correct answer. Results show recovery, evidentiary strength, public record, trust/exchange history, and deadline use rather than one opaque score.

## Cheap slice and acceptance criteria

Build no new runtime. Use a lightweight 12-minute case with one object, seven evidence records, two false leads, one lab slot, one human Crew Lead, one real LLM tool-using specialist, and a scripted rival pressure track. This cheaper experiment is not the proposed roster. It must exercise real model choice among current tool offers—not auto-succeed, replay a canned path, or substitute fake-agent output. Next test the intended two-specialist roster and compare House/BYO behavior.

Accept the slice only if:

- at least two evidence routes can support a deterministic final claim;
- the agent makes at least one result-dependent tool branch and cites the returned evidence IDs;
- the Crew Lead makes at least two consequential Role decisions, including the final commitment;
- a confident but unsupported narrative fails the Pack's evidence check;
- timeout, no Action, stale evidence, and tool failure have explicit visible consequences;
- a replay can explain the result from recorded Actions and evidence without calling a model.

Reject or redesign if playtesters mostly read long documents, answer multiple-choice trivia, rubber-stamp the agent, wait for prose, cannot explain why evidence passed, or discover one dominant call sequence. Also reject if a scripted helper performs as well as the tool-using agent at materially lower cost: the flagship should earn its agent complexity. Art recovery is a proposed creative direction, not a requirement to exclude fictional infiltration or action-heist mechanics from future Packs.

## Art, names, and licensing

Historical facts do not establish reuse rights for associated photographs, site prose, logos or other supplied assets. The Gardner Museum states that its downloadable collection images are generally licensed for educational, personal, non-commercial use under **CC BY-NC-ND 4.0**, with separate handling for commercial use. [Gardner rights and reproductions](https://www.gardnermuseum.org/organization/rights-reproductions)

Use original fictional names, objects, architecture, documents, portraits, and key art. If collection material is useful for mood boards or shipped assets, select records individually marked **CC0** from a source such as Smithsonian Open Access and retain asset-level provenance; the Smithsonian notes that CC0 addresses copyright but not necessarily trademark, privacy, publicity, or other third-party rights. [Smithsonian Open Access FAQ](https://www.si.edu/openaccess/faq)

This is a practical content guideline, not legal advice. Perform an asset-by-asset rights review before publication.
