import Image from "next/image";

const releaseUrl = "https://github.com/Magnus-Gille/sagascript/releases/latest";
const sourceUrl = "https://github.com/Magnus-Gille/sagascript";

export default function Home() {
  return (
    <main>
      <header className="site-header">
        <a className="wordmark" href="#top" aria-label="Sagascript home">
          <span className="mark" aria-hidden="true" />
          <span>Sagascript</span>
        </a>
        <nav aria-label="Main navigation">
          <a href="#how">How it works</a>
          <a href="#cli">CLI</a>
          <a href={sourceUrl}>GitHub</a>
        </nav>
      </header>

      <section className="hero" id="top">
        <div className="hero-copy">
          <p className="eyebrow"><span /> Local dictation for macOS and Windows</p>
          <h1>Speak. Get text.<br />Keep it local.</h1>
          <p className="lede">
            Sagascript turns your voice into text in your apps. Press your
            shortcut, speak, and the words appear where you are working.
          </p>
          <div className="hero-actions">
            <a className="button primary" href={releaseUrl}>Download for Mac <span aria-hidden="true">↘</span></a>
            <a className="button secondary" href="https://github.com/Magnus-Gille/sagascript/releases">Download for Windows <span aria-hidden="true">↘</span></a>
            <a className="button secondary" href={sourceUrl}>View source on GitHub <span aria-hidden="true">↗</span></a>
          </div>
          <p className="requirements">Mac: Apple silicon · macOS 13 or later. Windows: unsigned installers; check architecture and channel in the release notes.</p>
          <p className="microphone-note">
            Bluetooth headset microphones may give less reliable dictation,
            especially with fast speech. If words are missed, try
            a built-in microphone, if available, or a wired microphone.
          </p>
        </div>

        <figure className="product-shot">
          <div className="shot-frame">
            <Image
              src="/sagascript-app.png"
              alt="Sagascript settings showing Swedish and English dictation profiles with their shortcuts"
              width={612}
              height={772}
              priority
            />
          </div>
          <figcaption><span>01</span> Multiple languages. A shortcut for each. No cloud account.</figcaption>
        </figure>
      </section>

      <section className="statement" aria-label="Privacy statement">
        <p>Your recordings stay on your computer.</p>
        <span>
          Transcription happens locally. Internet access is only used when you
          choose a language that needs its speech engine, download another
          engine, or check for an update.
        </span>
      </section>

      <section className="release-news" id="releases" aria-labelledby="release-heading">
        <p className="eyebrow"><span /> Version 1.2.0</p>
        <h2 id="release-heading">More control over dictation.</h2>
        <p>Set independent push-to-talk and toggle shortcuts for each language profile.
          Save or discard dictionary edits explicitly. Finnish is now available with
          multilingual Whisper Base and an optional FinnishNLP Tiny engine.</p>
        <p>Transcription stays local. Meeting mode is planned for the next release.</p>
        <a href="https://github.com/Magnus-Gille/sagascript/releases/tag/v1.2.0">Read the full release notes →</a>
      </section>

      <section className="how" id="how">
        <div className="section-heading">
          <p className="eyebrow"><span /> How it works</p>
          <h2>Ready in minutes.</h2>
        </div>
        <ol className="steps">
          <li>
            <span className="step-number">01</span>
            <h3>Install</h3>
            <p>Mac: open the DMG and drag Sagascript to Applications. Windows: choose the installer for your architecture. Unsigned Windows installers may show a SmartScreen warning.</p>
          </li>
          <li>
            <span className="step-number">02</span>
            <h3>Choose a language</h3>
            <p>Sagascript prepares the right local speech engine and guides you through the required permissions.</p>
          </li>
          <li>
            <span className="step-number">03</span>
            <h3>Start speaking</h3>
            <p>Use your profile shortcut in any app. Add another shortcut when you need another language.</p>
          </li>
        </ol>
      </section>

      <section className="cli" id="cli">
        <div className="cli-copy">
          <p className="eyebrow light"><span /> CLI included</p>
          <h2>The same engine,<br />ready for scripts.</h2>
          <p>
            Transcribe recordings and folders from Terminal. The desktop app
            and CLI share the same private, local transcription engine.
          </p>
          <a href={`${sourceUrl}#cli-usage`}>Explore CLI commands <span aria-hidden="true">→</span></a>
        </div>
        <div className="terminal" aria-label="Sagascript command line examples">
          <div className="terminal-bar"><span /><span /><span /><b>Terminal</b></div>
          <pre><code><em>$</em> sagascript transcribe recording.mp3{"\n"}{"\n"}<span>Transcribing locally…</span>{"\n"}Done: recording.txt{"\n"}{"\n"}<em>$</em> sagascript --version</code></pre>
        </div>
      </section>

      <section className="final-cta">
        <p className="eyebrow"><span /> Sagascript for macOS and Windows</p>
        <h2>Your voice.<br />Your words.<br />Your machine.</h2>
        <a className="button primary" href={releaseUrl}>Download the latest release <span aria-hidden="true">↘</span></a>
      </section>

      <footer>
        <div className="wordmark"><span className="mark" aria-hidden="true" /><span>Sagascript</span></div>
        <p>Built by <a href="https://gille.ai/">Gille AI</a>.</p>
        <div><a href={sourceUrl}>GitHub</a><a href={`${sourceUrl}/blob/main/docs/installation.md`}>Installation</a></div>
      </footer>
    </main>
  );
}
