import test from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const script = fileURLToPath(new URL('./accept_live_local_message.mjs', import.meta.url))

test('unactivated production run stops before reading state or calling daemon/API', () => {
  const result = spawnSync(process.execPath, [script, 'run', '/nonexistent-private-acceptance-dir'], {
    encoding: 'utf8', env: { ...process.env, JCODE_PRODUCTION_LOCAL_ACTIVATED: '' }
  })
  assert.equal(result.status, 1)
  assert.equal(result.stdout, '')
  assert.match(result.stderr, /stopped at activation-gate/)
  assert.doesNotMatch(result.stderr, /ENOENT|Bearer|api_key/)
})

test('script parses without executing acceptance', () => {
  const result = spawnSync(process.execPath, ['--check', script], { encoding: 'utf8' })
  assert.equal(result.status, 0)
})

test('normal wire setup creates a new session and consumes actual legacy identity event', () => {
  const source = readFileSync(script, 'utf8')
  assert.match(source, /type: 'subscribe', id: 1/)
  assert.match(source, /continue_on_disconnect: true/)
  assert.match(source, /event.type === 'session'/)
  assert.doesNotMatch(source, /target_session_id:/)
  assert.doesNotMatch(source, /is_debug\s*=/)
})

test('single enqueue is persisted before POST and route remains original session', () => {
  // Source invariant, not a replacement for real production acceptance.
  const source = readFileSync(script, 'utf8')
  const attempted = source.indexOf("state.phase = 'enqueue_attempted'")
  const persist = source.indexOf('persist(state)', attempted)
  const enqueue = source.indexOf("await api('/mobile/commands',")
  assert(attempted > 0 && persist > attempted && enqueue > persist)
  assert.equal(source.match(/await api\('\/mobile\/commands',/g)?.length, 1)
  assert.match(source, /kind: 'message', session_id: state.session_id, text: state.prompt/)
  assert.doesNotMatch(source, /kind: ['"](?:transfer|create)['"]|\/cloud\/|\/billing\//)
})
