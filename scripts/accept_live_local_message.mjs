#!/usr/bin/env node
// Real own-account acceptance. No mock host, cloud attach, transfer or POST retry.
import { execFileSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { mkdirSync, readFileSync, writeFileSync, statSync } from 'node:fs'
import { homedir } from 'node:os'
import { resolve, join } from 'node:path'
import assert from 'node:assert/strict'

const [mode, directory] = process.argv.slice(2)
assert(['prepare', 'warm', 'check', 'run', 'cleanup'].includes(mode) && directory,
  'Usage: node scripts/accept_live_local_message.mjs prepare|warm|check|run|cleanup PRIVATE_DIRECTORY')
const dir = resolve(directory)
const binary = join(homedir(), '.local/bin/jcode')
const statePath = join(dir, 'acceptance.json')
const sleep = ms => new Promise(r => setTimeout(r, ms))
const debug = (command, id) => execFileSync(binary,
  ['debug', '--no-update', '--quiet', ...(id ? ['--session', id] : []), command],
  { encoding: 'utf8', timeout: 120000, stdio: ['ignore', 'pipe', 'pipe'] }).trim()
const persist = state => writeFileSync(statePath, JSON.stringify(state, null, 2) + '\n', { mode: 0o600 })
const textOf = message => typeof message.content === 'string' ? message.content :
  (message.content ?? []).filter(b => b.type === 'text').map(b => b.text).join('\n')
const prompt = token => `Reply with exactly ${token} and nothing else. Do not use any tools.`

async function main() {
  let state
  if (mode === 'prepare') {
    mkdirSync(dir, { recursive: true, mode: 0o700 })
    assert.equal(statSync(dir).mode & 0o077, 0, 'Scratch directory must be owner-only')
    try { readFileSync(statePath); throw new Error('Directory already has acceptance state') }
    catch (e) { if (e.code !== 'ENOENT') throw e }
    mkdirSync(join(dir, 'workspace'), { mode: 0o700 })
    const created = JSON.parse(debug(`create_session:${join(dir, 'workspace')}`))
    state = { version: 1, session_id: created.session_id, working_dir: join(dir, 'workspace'),
      created_at: new Date().toISOString(), warm_token: `LOCAL_WARM_${randomUUID()}`,
      token: `PHONE_LOCAL_${randomUUID()}`, phase: 'created' }
    persist(state)
    // Real minimal inference checks this dedicated session's inherited provider/model,
    // persists its transcript, and lets the ordinary daemon uploader publish it.
    debug(`message:${prompt(state.warm_token)}`, state.session_id)
    state.phase = 'prepared'
    persist(state)
    console.log(JSON.stringify({ phase: state.phase, session_id: state.session_id, directory: dir }))
    return
  }
  state = JSON.parse(readFileSync(statePath, 'utf8'))
  assert.equal(state.version, 1)
  assert.equal(state.working_dir, join(dir, 'workspace'))
  assert.match(state.session_id, /^session_[a-zA-Z0-9_-]+$/)
  const sessions = JSON.parse(debug('sessions'))
  const local = sessions.find(s => s.session_id === state.session_id)
  assert(local, 'Dedicated session is not registered in actual daemon')
  assert.equal(local.working_dir, state.working_dir)
  if (mode === 'warm') {
    assert.equal(state.phase, 'created', 'Warm recovery is only for an unprepared dedicated session')
    debug(`message:${prompt(state.warm_token)}`, state.session_id)
    assert.equal(debug('last_response', state.session_id), state.warm_token)
    state.phase = 'prepared'
    persist(state)
    console.log(JSON.stringify({ phase: state.phase, session_id: state.session_id }))
    return
  }
  if (mode === 'cleanup') {
    assert.equal(state.phase, 'passed', 'Retain failed/pending session and evidence for diagnosis')
    debug(`destroy_session:${state.session_id}`)
    state.phase = 'cleaned'
    persist(state)
    console.log(JSON.stringify({ phase: state.phase, session_id: state.session_id }))
    return
  }
  assert.equal(statSync(join(dir, 'auth.json')).mode & 0o077, 0, 'Auth config must be owner-only')
  const auth = JSON.parse(readFileSync(join(dir, 'auth.json'), 'utf8'))
  assert.equal(auth.api_base, 'https://api.jcode.sh/v1')
  assert.equal(typeof auth.api_key, 'string')
  assert(auth.api_key.length > 0)
  const requests = []
  const api = async (path, body) => {
    assert(path === `/mobile/sessions/${state.session_id}` ||
      path.startsWith(`/mobile/sessions/${state.session_id}?after_seq=`) ||
      path === '/mobile/commands' && body?.kind === 'message' && body?.session_id === state.session_id ||
      path === `/mobile/commands/${state.command_id}`, 'Endpoint outside scoped acceptance allowlist')
    requests.push({ method: body ? 'POST' : 'GET', path, at: new Date().toISOString() })
    const response = await fetch(auth.api_base + path, { method: body ? 'POST' : 'GET',
      headers: { Authorization: `Bearer ${auth.api_key}`, 'Content-Type': 'application/json' },
      ...(body ? { body: JSON.stringify(body) } : {}), signal: AbortSignal.timeout(20000), redirect: 'error' })
    assert(response.ok, `Production API HTTP ${response.status}`)
    return response.json()
  }
  const snapshot = async () => {
    let after = -1
    const messages = []
    for (let page = 0; page < 100; page++) {
      const data = await api(`/mobile/sessions/${state.session_id}?after_seq=${after}`)
      assert.equal(data.session.session_id, state.session_id)
      assert.equal(data.session.source, 'local')
      assert.equal(data.session.cwd, state.working_dir)
      messages.push(...data.messages)
      if (!data.more) return { session: data.session, messages }
      assert(data.messages.length && data.messages.at(-1).seq > after, 'Invalid paging cursor')
      after = data.messages.at(-1).seq
    }
    throw new Error('Transcript pagination exceeded bound')
  }
  const initial = await snapshot()
  assert.equal(initial.session.live, true, 'Dedicated session is not live on production API')
  assert.equal(debug('last_response', state.session_id), state.warm_token, 'Warm inference did not complete exactly')
  assert(initial.messages.some(m => m.role === 'assistant' && m.text === state.warm_token), 'Warm reply not uploaded by real daemon')
  if (mode === 'check') {
    console.log(JSON.stringify({ phase: 'ready', session_id: state.session_id, live: true,
      model: local.model, provider: local.provider, synced_messages: initial.messages.length }))
    return
  }
  assert.equal(process.env.JCODE_PRODUCTION_LOCAL_ACTIVATED, 'yes', 'Await root activation before setting JCODE_PRODUCTION_LOCAL_ACTIVATED=yes')
  assert.equal(state.phase, 'prepared', 'Never repeat an attempted enqueue, inspect saved evidence instead')
  state.phase = 'enqueue_attempted'
  state.prompt = prompt(state.token)
  persist(state) // Persist BEFORE POST, including ambiguous network failures. Never auto-retry.
  const result = await api('/mobile/commands', { kind: 'message', session_id: state.session_id, text: state.prompt })
  const command = result.command
  assert(command?.command_id)
  state.command_id = command.command_id
  persist(state)
  const validateCommand = c => {
    assert.equal(c.session_id, state.session_id)
    assert.equal(c.kind, 'message')
    assert.equal(c.target, 'local')
    assert(!c.result_session_id || c.result_session_id === state.session_id, 'Unexpected transferred session')
    assert(['queued', 'running', 'done', 'failed'].includes(c.status))
    assert.notEqual(c.status, 'failed', 'Production command failed')
    return c
  }
  validateCommand(command)
  const deadline = Date.now() + 240000
  let completed = false
  while (Date.now() < deadline) {
    const c = validateCommand((await api(`/mobile/commands/${state.command_id}`)).command)
    if (c.status === 'done') { completed = true; break }
    await sleep(2000)
  }
  assert(completed, 'Command completion timed out')
  // done only acknowledges delivery. Independently await actual inference and upload.
  let final
  while (Date.now() < deadline) {
    final = await snapshot()
    if (final.messages.some(m => m.role === 'assistant' && m.text === state.token) &&
        debug('last_response', state.session_id) === state.token) break
    await sleep(2000)
  }
  const history = JSON.parse(debug('history', state.session_id))
  assert(history.some(m => m.role === 'user' && textOf(m) === state.prompt), 'Prompt missing from same local history')
  assert(history.some(m => m.role === 'assistant' && textOf(m) === state.token), 'Exact reply missing from local history')
  assert(final.messages.some(m => m.role === 'user' && m.text === state.prompt))
  assert(final.messages.some(m => m.role === 'assistant' && m.text === state.token))
  assert(!final.messages.some(m => m.role === 'tool'), 'Unexpected synced tool usage')
  assert(!history.some(m => Array.isArray(m.content) && m.content.some(b => ['tool_use', 'tool_result'].includes(b.type))), 'Unexpected tool usage')
  const evidence = { at: new Date().toISOString(), session_id: state.session_id, command_id: state.command_id,
    target: 'local', command_status: 'done', same_local_session: true, source: final.session.source,
    model: local.model, provider: local.provider, token: state.token, requests, history, synced: final }
  writeFileSync(join(dir, 'evidence.json'), JSON.stringify(evidence, null, 2) + '\n', { mode: 0o600 })
  state.phase = 'passed'
  persist(state)
  console.log(JSON.stringify({ phase: 'passed', session_id: state.session_id, command_id: state.command_id,
    evidence: join(dir, 'evidence.json') }))
}
main().catch(() => {
  // Never print raw HTTP bodies, config, child stderr, or credential-bearing exceptions.
  console.error('Acceptance stopped. No automatic enqueue retry. Inspect private state and dedicated session. See documentation for checkpoints.')
  process.exitCode = 1
})
