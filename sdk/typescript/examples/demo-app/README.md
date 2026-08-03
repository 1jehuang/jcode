# jcode SDK demo app

A standalone Node application that launches a private jcode agent, streams its
reply, reports tool calls, and cleans up the private instance on exit.

## Run it

Install [jcode](https://jcode.sh), sign in to at least one model provider, then:

```bash
npm install
npm start -- "Summarize this project"
```

The SDK inherits your existing jcode provider logins by default. The private
instance has separate sessions and state, and `client.close()` removes it when
the application exits.

Set `JCODE_BINARY=/full/path/to/jcode` when testing a particular local build.
For a production application, handle permission requests explicitly instead of
using `autoApprove: true`.
