# Native Astra design review

Date: 2026-10-12.

A separate OMP invocation requested `openai-codex/gpt-6-astra` with the approved Ironbark screenshot attached. It used low reasoning, no tools, no extensions/skills/rules, no saved session, and a 120-second model-run limit. The native provider had an existing credential. The call returned substantive design feedback in 13.36 seconds; this was not inferred from exit status alone.

This does not mean the top-level conversation switched models, that Astra authored the HTML, or that Codex WebGPT was activated. The requested route was native `openai-codex`, not the inactive WebGPT bridge.

## Returned feedback

> **Keep the approved layout.** It supports a conversation-first workstation interface without dashboard ornament.
>
> 1. **Preserve the visual hierarchy:** compact left navigation, soot surfaces, parchment text, restrained amber selection, and original forest artwork anchored below navigation. The artwork gives Ironbark identity without competing with the conversation.
>
> 2. **Keep orchestration secondary to the conversation.** The inline task strip and collapsible “Task details” rail expose assignments, workspace isolation, and approval boundaries without becoming the main screen. The displayed running/completed states are sample states—not evidence of live agents or integrations.
>
> 3. **Retain separate provider and harness selectors in the composer.** “Codex WebGPT (not connected)” beside “OMP” makes that distinction visible. Use the existing Knowledge navigation for reference material and the task/context surfaces for local JEV judgments; neither needs a new dashboard. JEV operation is not demonstrated by this mockup.

Outcome: retain the user-approved visual direction. No production feature or permission change follows merely from this review.
