# MAS Skill Pack

This repository includes a small project-local skill pack adapted from MAS/Codex engineering workflows. The skills live under `.jcode/skills/`, so jcode can load them as project-local skills without adding runtime dependencies.

## Included Skills

| Skill | Purpose |
|---|---|
| `search-first` | Research existing repository patterns, tools, skills, MCP integrations, and dependencies before adding new code. |
| `verification-loop` | Close implementation work with deliberate checks, diff review, and residual-risk reporting. |
| `promptify` | Convert raw or vague engineering requests into structured, anti-hallucination prompts with acceptance criteria. |
| `network-traffic-drops` | Diagnose packet drops across NIC, bond, TAP, softnet, OVS, and OVN paths with ranked mitigations. |

## Why Project-Local Skills

Project-local skills keep the change low-risk:

- They use jcode's existing `.jcode/skills/<skill-name>/SKILL.md` loading path.
- They do not change providers, memory, MCP, swarm, browser, safety, or server behavior.
- They can be activated manually with slash commands such as `/search-first`.
- They can also participate in jcode's existing skill discovery and semantic injection behavior.

## Usage

List or inspect skills from inside jcode with the skill tool:

```text
skill_manage action=list
skill_manage action=read name=promptify
```

Manual activation examples:

```text
/search-first
/verification-loop
/promptify
/network-traffic-drops
```

## Maintenance Notes

Keep each skill self-contained and small. If a workflow needs scripts, fixtures, or long references, place those under the corresponding skill directory and reference them from `SKILL.md` instead of expanding the always-loaded skill body.

When changing skills, verify that every file has valid YAML frontmatter with at least:

```yaml
---
name: skill-name
description: What triggers the skill and what it does.
---
```
