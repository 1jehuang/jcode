# Bedrock default routing

The built-in default is `anthropic.claude-opus-5-5`. The bare model name is
retained as a logical default until request time, when the effective AWS SDK
client region is known (including regions loaded from AWS profiles).

| Client region | Default request model |
| --- | --- |
| `eu-west-2` | `anthropic.claude-opus-5-5` (in-region) |
| `us-east-1`, `us-east-2`, `us-west-1`, `us-west-2` | `us.anthropic.claude-opus-5-5` |
| `eu-central-1`, `eu-central-2`, `eu-north-1`, `eu-south-1`, `eu-south-2`, `eu-west-1`, `eu-west-3` | `eu.anthropic.claude-opus-5-5` |
| `ap-southeast-2`, `ap-southeast-4` | `au.anthropic.claude-opus-5-5` |
| `ap-northeast-1`, `ap-northeast-3` | `jp.anthropic.claude-opus-5-5` |
| Other or missing region | Actionable error, no automatic global fallback |

Geographic profiles may route across regions within their geography. They do
not promise single-region residency. Canada is deliberately not mapped to the
US profile, because that profile can send Canadian requests to US regions.
GovCloud, new regions and global-only regions require an explicit model/profile
choice. Existing explicit profiles, application profile ARNs and non-default
model choices are preserved. An explicit `global.` profile opts into worldwide
routing. Cached catalog routes cannot replace the bare default with a foreign
or global Opus 5.5 profile.

Opus 5.5 requests omit temperature and top-p overrides, retain token limits and
stop sequences, and use low effort by default. `JCODE_BEDROCK_EFFORT` or the
provider effort setter can select `low`, `medium`, `high`, `xhigh`, or `max`.
The setter's `default` choice restores low. Effort is sent through Converse's
`additionalModelRequestFields.output_config.effort`; older models do not receive
this field. Unknown effort values fail before dispatch. Application profile
ARNs with opaque IDs cannot be identified as Opus 5.5, so model-specific metadata,
sampling filtering and low-effort defaults cannot be inferred for those ARNs.

Reference documentation checked October 20, 2026:

- https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-anthropic-claude-opus-5-5.html
- https://docs.aws.amazon.com/bedrock/latest/userguide/claude-messages-adaptive-thinking.html
- https://platform.claude.com/docs/en/models/opus-5-5/migration-guide

Unit tests exercise routing, metadata, overrides and request configuration
without contacting AWS. The ignored live smoke test requires explicit AWS
credentials/model access. Unit tests do not establish account permissions,
quotas, live model availability or successful multi-turn inference.
