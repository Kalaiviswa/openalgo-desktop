/**
 * Desktop: the OpenScript live runner, one run per page.
 *
 * The desktop runs a deployed strategy on the same `openalgo-script` engine the
 * chart and the backtest use, in a hidden window the app opens for the run, so
 * backtest and live run one engine and cannot drift. This file is the driver
 * the web's Python runner (`openscript_host/openscript_runner.py`) is, on the
 * TypeScript engine:
 *
 * - **The history pass sends nothing and the run begins flat.** Every bar
 *   already closed is replayed to bring the engine to the present; an order the
 *   script decides during it is answered `rejected` here and never leaves the
 *   page, so no position is invented and no old signal is traded.
 * - **Orders are sent from a confirmed bar only.** A moving bar is executed
 *   again on every change at the same index (the engine rolls its state back);
 *   an order a script routes from an unconfirmed execution is answered
 *   `rejected` with the reason, which is what its backtest did.
 * - **The page decides nothing about money.** It holds no key and no session:
 *   it hands each confirmed bar's intents to the app, which turns them into
 *   orders on the run's own side, tags them with the deployment and reports
 *   every order back as a frame. A Stop's closing order is the app's own.
 * - **A lost answer is not a refusal** (NEW-TR-04). When the app's answer to
 *   a batch does not come back, or cannot be read, the order may have been
 *   placed: the batch stays pending, nothing new is sent, and the same batch
 *   is sent again until the app answers. The app answers an intent it has
 *   handled already from its record and never places it twice. Only the app
 *   saying no rejects a batch here.
 * - **Only this run's frames** (NEW-TR-03). Each start of a deployment is a
 *   new engine whose intents are numbered from 1 again; every message from
 *   the app names the run it is for, and one for another run is dropped.
 */

import type {
  Engine,
  EngineHost,
  HostBar,
  LoadResult,
  OrderFrame,
  OrderIntent,
  RoutedEffect,
} from 'openalgo-script'
import { languageInterval } from '@/lib/trading/openscriptIntervals'

/** What the app serves the page about its run. */
export interface RunSpec {
  run: {
    id: string
    file: string
    symbol: string
    exchange: string
    interval: string
    /** This run's engine incarnation: the app's messages name it. */
    incarnation: number
  }
  program: string
  inputs: Record<string, unknown>
  facts: { instrument?: Record<string, unknown> } | null
}

/**
 * `halt` and `resume` are the two halves of a Stop. The app halts the page
 * before it closes the position, so the script sends nothing beside the
 * close. A close that did not happen and left nothing of its own working
 * resumes the page: the run is still holding its position, and its script is
 * what manages it, so it must be able to send its own exits again. A close
 * that left an order working sends no resume: the run stays halted (close
 * pending) until Stop is pressed again or the run is paused.
 */
export type InboxMessage = { seq: number; incarnation: number } & (
  | { kind: 'bars'; bars: HostBar[] }
  | { kind: 'frame'; frame: OrderFrame }
  | { kind: 'halt' }
  | { kind: 'resume' }
)

/**
 * The page's channel to the app. Every call is the run's own. A call that
 * fails throws: {@link RunGone} when the app no longer knows the run,
 * {@link Refused} when the app answered no, anything else when no readable
 * answer came back (which says nothing about what the app did).
 */
export interface Transport {
  spec(): Promise<RunSpec>
  bars(): Promise<HostBar[]>
  inbox(after: number): Promise<{ messages: InboxMessage[]; stop: boolean }>
  intents(incarnation: number, intents: readonly OrderIntent[]): Promise<void>
  /** With the page's state: a batch pending an answer (`waiting`, shown in
   * the run status), and pending long enough that the trader must be told
   * (`stuck`, a Health alert). */
  log(lines: readonly string[], state: PageState): Promise<void>
  ended(message: string): Promise<void>
}

/** The engine's loader, injected so the driver can be tested on its own. */
export type Loader = (
  text: string,
  options: { settings: Record<string, unknown>; host: EngineHost }
) => LoadResult

const UNIT_MS: Record<string, number> = {
  m: 60_000,
  h: 3_600_000,
  D: 86_400_000,
  W: 604_800_000,
  M: 2_592_000_000,
}

/** One bar's length in milliseconds, from a broker interval code. */
export function intervalMs(code: string): number | null {
  const spelled = languageInterval(code)
  if (spelled === null) return null
  const found = /^([0-9]+)(m|h|D|W|M)?$/.exec(spelled)
  if (found === null) return null
  return Number.parseInt(found[1], 10) * UNIT_MS[found[2] ?? 'm']
}

export const REPLAY_REFUSAL = 'replayed from history, so it was not sent'
export const MOVING_REFUSAL =
  'orders are sent when a bar closes, which is what its backtest did, so an intrabar call is not sent'
export const HALTED_REFUSAL = 'this run is stopping, so nothing further is sent'
export const WAITING_REFUSAL =
  'an earlier order of this run has not been confirmed by the app yet, so nothing new is sent'
/** How long the page waits before asking the app again about a batch it
 * has no answer for, attempt by attempt; the last delay repeats. */
export const RETRY_DELAYS_MS: readonly number[] = [1_000, 2_000, 5_000, 10_000]
/** After this many asks without an answer (about a minute), the trader is
 * told: the run's own exits are held back too while the batch is pending. */
export const STUCK_AFTER = 8

/** What the page tells the app with its log lines. */
export interface PageState {
  waiting: boolean
  stuck: boolean
}

export class Driver {
  private engine: Engine | null = null
  private routed: OrderIntent[] = []
  private lastConfirmed: number | null = null
  private moving: HostBar | null = null
  private sending = false
  private halted = false
  private stopped = false
  /** A batch the app has not answered: it may have been placed. */
  private waiting: OrderIntent[] | null = null
  private retries = 0
  private stuckTold = false
  /** The state the app last heard (see `flush`). */
  private reported: PageState = { waiting: false, stuck: false }
  private retryTimer: ReturnType<typeof setTimeout> | null = null
  private asking = false
  private currentTime = 0
  private readonly lines: string[] = []
  private readonly barMs: number | null
  private readonly transport: Transport
  private readonly loader: Loader
  private readonly spec: RunSpec
  private readonly now: () => number

  constructor(
    transport: Transport,
    loader: Loader,
    spec: RunSpec,
    now: () => number = () => Date.now()
  ) {
    this.transport = transport
    this.loader = loader
    this.spec = spec
    this.now = now
    this.barMs = intervalMs(spec.run.interval)
  }

  get isStopped(): boolean {
    return this.stopped
  }

  get isSending(): boolean {
    return this.sending
  }

  /** Whether a batch is pending an answer from the app. */
  get isWaiting(): boolean {
    return this.waiting !== null
  }

  say(line: string): void {
    this.lines.push(line)
  }

  /**
   * Send the run's log lines, and the page's state whenever it changed: the
   * app shows a pending batch in the run status and raises the Health alert
   * from it, so a change goes out even with nothing to log.
   */
  async flush(): Promise<void> {
    const state: PageState = { waiting: this.isWaiting, stuck: this.stuckTold }
    const changed = state.waiting !== this.reported.waiting || state.stuck !== this.reported.stuck
    if (this.lines.length === 0 && !changed) return
    const out = this.lines.splice(0, this.lines.length)
    try {
      await this.transport.log(out, state)
      this.reported = state
    } catch {
      // Lines are a convenience; the state is sent again with the next flush.
    }
  }

  /** Load the program. False when the engine refused it. */
  load(): boolean {
    const { run, facts } = this.spec
    const instrument = {
      ...(facts?.instrument ?? {}),
      symbol: run.symbol,
      exchange: run.exchange,
      interval: languageInterval(run.interval) ?? run.interval,
    }
    const self = this
    const host: EngineHost = {
      instrument,
      // The bar's own open time, never the wall clock: two executions of one
      // moving bar must see the same moment.
      get now() {
        return self.currentTime
      },
      route: (effect: RoutedEffect) => {
        for (const intent of effect.intents) this.routed.push(intent)
      },
    }
    const loaded = this.loader(this.spec.program, { settings: this.spec.inputs, host })
    if (!loaded.ok) {
      this.stop(
        `${run.file} could not be loaded: ${loaded.diagnostic.code} ${loaded.diagnostic.message}`
      )
      return false
    }
    this.engine = loaded.engine
    return true
  }

  private stop(message: string): void {
    if (this.stopped) return
    this.stopRetrying()
    this.stopped = true
    this.say(message)
    void this.flush().then(() => this.transport.ended(message).catch(() => undefined))
  }

  private reject(intents: readonly OrderIntent[], text: string): void {
    for (const intent of intents) {
      this.engine?.deliver({ intentId: intent.intentId, status: 'rejected', filledQty: 0, text })
    }
  }

  /** Whether a bar has closed by the clock. */
  private closedByClock(bar: HostBar): boolean {
    if (this.barMs === null || bar.time === null) return false
    return bar.time + this.barMs <= this.now()
  }

  /** One execution. Sends what a confirmed, live execution routed. */
  private async execute(bar: HostBar, isNew: boolean, confirmed: boolean, supplied?: number) {
    const engine = this.engine
    if (engine === null || this.stopped) return
    this.routed = []
    this.currentTime = bar.time ?? 0
    const state = { isConfirmed: confirmed, isRealtime: this.sending }
    const result = isNew ? engine.append(bar, state, supplied) : engine.update(bar, state)
    const routed = this.routed
    this.routed = []
    if (result.diagnostic !== undefined) {
      this.reject(routed, 'the script stopped on this bar')
      this.stop(
        `${this.spec.run.file} stopped on bar ${result.index}: ${result.diagnostic.code} ${result.diagnostic.message}. Nothing further will be sent for it.`
      )
      return
    }
    for (const alert of result.alerts) {
      this.say(`Alert ${alert.key}: ${alert.title} ${String(alert.message ?? '')}`)
    }
    if (routed.length === 0) return
    if (!this.sending) return this.reject(routed, REPLAY_REFUSAL)
    if (!confirmed) return this.reject(routed, MOVING_REFUSAL)
    if (this.halted) return this.reject(routed, HALTED_REFUSAL)
    if (this.waiting !== null) return this.reject(routed, WAITING_REFUSAL)
    await this.submit(routed)
  }

  /**
   * Hand a batch to the app. The app saying no rejects it; no readable
   * answer at all leaves it pending, because the app may have placed it.
   */
  private async submit(batch: OrderIntent[]): Promise<void> {
    try {
      await this.transport.intents(this.spec.run.incarnation, batch)
    } catch (e) {
      if (e instanceof RunGone) return this.gone()
      if (e instanceof Refused) return this.reject(batch, e.message)
      this.waiting = batch
      this.retries = 0
      this.say(
        `The app's answer to ${batchName(batch)} did not come back (${e instanceof Error ? e.message : String(e)}). It may have been placed, so nothing new is sent until the app confirms it. Asking again.`
      )
      void this.flush()
      this.scheduleRetry()
    }
  }

  private scheduleRetry(): void {
    if (this.stopped || this.waiting === null || this.retryTimer !== null) return
    const delay = RETRY_DELAYS_MS[Math.min(this.retries, RETRY_DELAYS_MS.length - 1)]
    this.retries += 1
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null
      void this.askAgain()
    }, delay)
  }

  private stopRetrying(): void {
    if (this.retryTimer !== null) clearTimeout(this.retryTimer)
    this.retryTimer = null
  }

  /**
   * Send the pending batch again, unchanged. The app answers each intent it
   * handled already from its record and places nothing twice; its frames
   * follow in the inbox.
   */
  async askAgain(): Promise<void> {
    const batch = this.waiting
    if (batch === null || this.stopped || this.asking) return
    this.asking = true
    try {
      await this.transport.intents(this.spec.run.incarnation, batch)
      this.waiting = null
      this.stuckTold = false
      this.say(`The app confirmed it has ${batchName(batch)}. This run sends orders again.`)
    } catch (e) {
      if (e instanceof RunGone) return this.gone()
      if (e instanceof Refused) {
        this.waiting = null
        this.stuckTold = false
        this.reject(batch, e.message)
        this.say(`The app refused ${batchName(batch)}: ${e.message}. This run sends orders again.`)
      } else {
        if (this.retries >= STUCK_AFTER && !this.stuckTold) {
          this.stuckTold = true
          this.say(
            `The app's answer to ${batchName(batch)} has not come back for over a minute, so whether it was placed is not known. Nothing new is sent, this run's own exits included, until it is. Check your order book.`
          )
        }
        this.scheduleRetry()
      }
    } finally {
      this.asking = false
    }
    await this.flush()
  }

  /** The app no longer knows this run: its work, and what it was waiting on, is the app's. */
  private gone(): void {
    this.stopRetrying()
    this.stopped = true
  }

  /** Replay history, then begin. */
  async begin(history: readonly HostBar[]): Promise<void> {
    const engine = this.engine
    if (engine === null) return
    const bars = [...history].sort((a, b) => (a.time ?? 0) - (b.time ?? 0))
    engine.history(bars)
    const last = bars[bars.length - 1]
    const forming = last !== undefined && !this.closedByClock(last)
    const closed = forming ? bars.slice(0, -1) : bars
    for (const bar of closed) {
      await this.execute(bar, true, true, bars.length)
      if (this.stopped) return
      this.lastConfirmed = bar.time
    }
    this.sending = true
    this.say(
      `Replayed ${closed.length} bars of history. Nothing was sent for them, and this run begins holding nothing.`
    )
    if (forming && last !== undefined) {
      this.moving = last
      await this.execute(last, true, false)
    }
  }

  /** New or changed bars from the app, oldest first. */
  async onBars(incoming: readonly HostBar[]): Promise<void> {
    const bars = [...incoming]
      .filter(
        (b) => b.time !== null && (this.lastConfirmed === null || b.time > this.lastConfirmed)
      )
      .sort((a, b) => (a.time ?? 0) - (b.time ?? 0))
    for (let i = 0; i < bars.length && !this.stopped; i += 1) {
      const bar = bars[i]
      const confirmed = i < bars.length - 1 || this.closedByClock(bar)
      if (this.moving !== null && this.moving.time === bar.time) {
        this.moving = confirmed ? null : bar
        await this.execute(bar, false, confirmed)
      } else {
        // A newer bar has begun: the one that was moving closed as last seen.
        await this.confirmMoving()
        if (this.stopped) return
        this.moving = confirmed ? null : bar
        await this.execute(bar, true, confirmed)
      }
      if (confirmed) this.lastConfirmed = bar.time
    }
  }

  /** Confirm the moving bar as last seen. */
  async confirmMoving(): Promise<void> {
    const bar = this.moving
    if (bar === null) return
    this.moving = null
    await this.execute(bar, false, true)
    this.lastConfirmed = bar.time
  }

  /** A moving bar whose time is up is closed even when no newer bar came. */
  async onClock(): Promise<void> {
    if (this.moving !== null && this.closedByClock(this.moving)) await this.confirmMoving()
  }

  deliver(frame: OrderFrame): void {
    this.engine?.deliver(frame)
  }

  halt(): void {
    if (!this.halted) this.say('This run was asked to stop. Nothing further is sent.')
    this.halted = true
  }

  /** The close did not happen: the script sends its own orders again. */
  resume(): void {
    if (this.halted) {
      this.say('The position was not closed, so this run carries on and sends its orders again.')
    }
    this.halted = false
  }

  /** Read the inbox until the app says stop. */
  async run(): Promise<void> {
    let after = 0
    while (!this.stopped) {
      let answer: { messages: InboxMessage[]; stop: boolean }
      try {
        answer = await this.transport.inbox(after)
      } catch (e) {
        if (e instanceof RunGone) {
          this.gone()
          return
        }
        await new Promise((r) => setTimeout(r, 2000))
        continue
      }
      for (const m of answer.messages) {
        after = Math.max(after, m.seq)
        // Another run's message: never this engine's, whatever its number.
        if (m.incarnation !== this.spec.run.incarnation) continue
        if (m.kind === 'bars') await this.onBars(m.bars)
        else if (m.kind === 'frame') this.deliver(m.frame)
        else if (m.kind === 'halt') this.halt()
        else if (m.kind === 'resume') this.resume()
      }
      await this.onClock()
      await this.flush()
      if (answer.stop) {
        this.gone()
        return
      }
    }
  }
}

/** The app no longer knows this run: the page's work is over. */
export class RunGone extends Error {
  constructor() {
    super('This strategy page is not running any more.')
    this.name = 'RunGone'
  }
}

/** The app answered a request with a refusal: nothing of it was done. */
export class Refused extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'Refused'
  }
}

/** A batch as the run's log names it. */
function batchName(batch: readonly OrderIntent[]): string {
  const ids = batch.map((i) => i.intentId).join(', ')
  return batch.length === 1 ? `order intent ${ids}` : `order intents ${ids}`
}
