# Device-aware and deferred deep linking for quark

Research date: 2026-07-14
Purpose: decide how quark builds the "open the native app" feature for a short link.
Audience: the owner. Read time: about 8 minutes. No code here, only a decision.

---

## TL;DR

A short link can open a native app instead of the web because the app publisher and
the link's domain have a signed association: iOS reads an `apple-app-site-association`
(AASA) file, Android reads `assetlinks.json`. quark already hosts these files, which is
the hard prerequisite. When the app is installed, the OS intercepts the tap and routes
it to the app with the full URL. When the app is not installed, the OS does nothing
special and the link falls back to a normal web request, where quark can send the user
to the App Store, Play Store, or a web page.

The genuinely hard part is *deferred* deep linking (app not installed, user installs,
and the first app open should still land on the original target). Doing that reliably
needs an in-app SDK and server-side matching. It is what Branch, AppsFlyer, and Adjust
sell. Google's own Firebase Dynamic Links, the free option most people used, shut down
on 2025-08-25 and now returns 404. Apple's privacy changes (ATT, the iOS 17
fingerprinting ban) broke the probabilistic matching these tools relied on, so even the
paid tools are less reliable on iOS than they were.

Recommendation for quark: ship a server-side, per-link, User-Agent-based redirect
(platform detection to app URL or store URL, with a web fallback). Do not attempt
deferred deep linking in v1. It needs a mobile SDK quark does not have and cannot get
as a redirect service.

---

## 1. The mechanics, plainly

A "deep link" is a URL that points at a specific place inside an app rather than a web
page. Getting a tap on an `https://` short link to open an app comes down to one thing:
the operating system has to already know that this domain is allowed to open that app.
That knowledge comes from an association file the domain hosts.

**iOS Universal Links.** The app declares which domains it handles. The domain hosts an
`apple-app-site-association` file (JSON, served at `/.well-known/apple-app-site-association`,
Content-Type `application/json`, no redirects, under 128 KB). It lists app IDs
(team ID + bundle ID) and the URL path patterns each app claims. When the app is
installed, iOS fetches this file and registers the association. After that, a tap on a
matching `https://` link is routed straight to the app with the full URL handed to it.
If the app is not installed, the same link is a normal web request and Safari loads it.

**Android App Links.** Same idea. The domain hosts `assetlinks.json` at
`/.well-known/assetlinks.json`. It contains the app's package name and the SHA-256
fingerprint of the app's signing certificate. Android verifies the association at app
install time and routes matching `https://` links into the app. No app installed means
a normal browser navigation.

**Custom URI schemes (legacy / fallback).** Before Universal Links and App Links, apps
registered schemes like `myapp://product/42`. These still work but have two problems:
any app can claim any scheme (no ownership proof, so they are hijackable), and if the
app is not installed the browser shows an ugly "cannot open page" error instead of
falling back cleanly. Today they are used only as a secondary attempt or inside code
that already knows the app is present. The `https://`-based Universal Links / App Links
are the primary mechanism because they are verified and they fall back to the web
gracefully.

**Why the association files are the prerequisite.** Without a valid AASA /
`assetlinks.json` pair, the OS will never hand the link to the app, full stop. Nothing
quark does on the redirect side can substitute for it. A single mistake breaks it
silently: an HTTP-to-HTTPS redirect on the `.well-known` path, the wrong Content-Type,
a stale certificate fingerprint. quark already hosts these files, so this box is
checked, but it is worth stating that this is app-publisher configuration, not
something a link can carry.

Sources:
https://www.thewidlarzgroup.com/blog/deep-links-universal-and-asset-links
https://www.airbridge.io/en/blog/universal-links-vs-app-links-cross-platform-guide
https://dev.to/marko_boras_64fe51f7833a6/universal-deep-links-2026-complete-guide-36c4

---

## 2. Deferred deep linking (the hard case: app not installed)

Universal Links and App Links only solve the case where the app is already installed.
Neither handles the "install first, then land on the right screen" journey. Once the
user is bounced to the App Store or Play Store, the original target is lost unless
something carries it across the install boundary. That something is deferred deep
linking.

The standard flow:

1. User taps the link. App is not installed.
2. Redirect sends them to the store, and the intended destination is stashed somewhere.
3. User installs and opens the app for the first time.
4. On first launch, the app asks a server "what link brought this device here?" and
   routes to that target.

The whole problem is step 3 to step 4: matching a fresh install back to the click that
caused it. The industry uses several mechanisms, in rough order of reliability:

- **Google Play Install Referrer API (Android, deterministic).** The Play Store can
  pass a referrer string that carries a click token into the freshly installed app,
  plus timestamps. This gives a definitive one-to-one match between click and install.
  It is the reliable path and the reason Android deferred linking works well (roughly
  98% accurate in practice).
  https://vmobify.com/blog/google-play-install-referrer

- **Apple SKAdNetwork / AdAttributionKit (iOS, deterministic but parameterless).**
  Apple's frameworks send delayed, aggregated postbacks for privacy-preserving install
  attribution. They deliberately carry no deep-link parameters and no user-level routing
  context. They tell you a campaign drove installs; they cannot tell an individual
  device where to land. So they do not solve deferred routing.
  https://warpdriven.ai/en/blog/industry-1/deferred-deep-link-attribution-pitfalls-direct-to-app-82

- **Clipboard.** The redirect writes the target to the clipboard; the app reads it on
  first launch. Works, but iOS now shows a paste notification and users can deny it, so
  it is fragile and increasingly user-visible.

- **Fingerprint / probabilistic matching (dying).** With no deterministic token, tools
  fell back to matching on IP address, device model, OS version, and screen size within
  a short time window. This used to hit 70 to 90%. Three changes broke it on iOS:
  App Tracking Transparency (2021), Apple's iOS 17 ban on fingerprinting for this
  purpose (enforced May 2024), and the parameterless design of SKAdNetwork. VPNs, IPv6
  rotation, and ATT opt-outs push accuracy down further. Post-ATT, fingerprinting on iOS
  is not something to build on.
  https://www.airbridge.io/en/blog/deferred-deeplink-post-idfa-accuracy
  https://deeplinknow.com/blog/deferred-deep-linking-2025

Net: deferred deep linking is reliable on Android (Install Referrer) and unreliable on
iOS (no deterministic parameter-carrying path, fingerprinting curtailed). Every option
except the clipboard requires code running inside the app on first launch. A redirect
service alone cannot do it.

---

## 3. How the big players do it

All four sit *on top* of Universal Links / App Links. The OS-level association is still
required; these products add the deferred behavior, attribution, and a hosted decision
layer. The common thread: they all need an SDK embedded in the app to close the deferred
loop and to read the install referrer / first-launch context.

**Branch.io.** The closest thing to a pure deep-linking product (the others bundle it
into attribution suites). You embed the Branch SDK. A Branch link resolves server-side:
Branch decides per click whether to open the app, send to a store, or show a web page,
and on install the SDK asks Branch for the deferred target. Fallback is a configurable
waterfall (app, then store, then web URL). Needs the SDK.
https://help.branch.io/marketer-hub/docs/branch-attribution-explained

**AppsFlyer OneLink.** Deep linking bundled with AppsFlyer's attribution platform.
OneLink routes existing users (app installed) into the app via App Links / Universal
Links / URI scheme, and routes new users (no app) to the store or a web URL. If the
in-app link handling is not set up, users fall back to the same path as new users
(store or web). Google officially names OneLink as a Firebase Dynamic Links replacement.
Needs the AppsFlyer SDK.
https://support.appsflyer.com/hc/en-us/articles/208874366-Create-deep-linking-and-redirection-links-for-your-campaigns-with-OneLink

**Adjust.** An MMP (mobile measurement partner) with deep linking included alongside
attribution and fraud detection. Same architecture as OneLink: SDK in the app, hosted
resolution, store-or-web fallback, deferred deep linking via Install Referrer on Android
and Apple's frameworks plus best-effort matching on iOS. Positioned for privacy-heavy
markets. Needs the Adjust SDK.
https://www.adjust.com/blog/switching-from-firebase/

**Firebase Dynamic Links (dead).** This was Google's free product and the default choice
for indie developers. Confirmed: it is shut down. The Firebase console went read-only on
2024-05-24 (no new links). The service was fully turned off on **2025-08-25**. Since then
all Dynamic Links (both custom domains and `page.link` subdomains) return HTTP 404, the
APIs return 400/403, and analytics data not exported before the date is gone. Google's
official guidance points to: third-party providers for full parity (it lists Adjust,
Airbridge, AppsFlyer, Bitly, Branch, Kochava, Singular); plain App Links / Universal
Links if you only need post-install routing and not deferred; or just removing the
feature. The relevant takeaway for quark: the free, easy option no longer exists, which
is exactly why teams are now either paying an MMP or building the simple server-side
version themselves.
https://firebase.google.com/support/dynamic-links-faq

---

## 4. The fallback waterfall

A device-aware redirect runs a decision tree. The realistic version:

```
Tap on short link
  |
  v
What is the User-Agent?
  |-- Desktop / bot / unknown  -> web URL
  |-- iOS
  |     app installed?  (OS decides via Universal Link, before quark is even hit)
  |       yes -> app opens directly, quark never sees the request
  |       no  -> quark serves: try App Store, or web URL
  |-- Android
        app installed?  (OS decides via App Link, before quark is hit)
          yes -> app opens directly, quark never sees the request
          no  -> quark serves: try Play Store, or web URL
```

Two things about this tree matter and are easy to get wrong:

**You cannot reliably detect "app installed" from a browser.** There is no API a web
server or web page can call to ask "is this app on this phone?". By design, for privacy.
When the app *is* installed and the association files are valid, the OS intercepts the
tap and quark never receives the request at all. When quark *does* receive the request,
that usually means the app is not installed (or the OS routing was bypassed). So quark's
job is mostly the "no app" branch. This is a feature, not a limitation to fight: let the
OS handle the installed case, and handle the not-installed case on the server.

**In-app browsers break OS routing.** Links opened inside Instagram, TikTok, Facebook,
or LinkedIn run in an embedded webview that bypasses Universal Link / App Link
interception on both platforms. In those contexts the app will not open even if
installed, and the user lands on whatever web fallback you configured. There is no clean
fix from the server side; the common mitigation is a landing page that tells the user to
open in the system browser, which is more UI than a redirect service should own.

Sources:
https://www.airbridge.io/en/blog/universal-links-vs-app-links-cross-platform-guide
https://support.appsflyer.com/hc/en-us/articles/208874366-Create-deep-linking-and-redirection-links-for-your-campaigns-with-OneLink

---

## 5. What quark can realistically do

quark's constraints: single binary, redirect service, the hot path (resolve short code
to destination and issue a 3xx) must stay cheap, no mobile SDK, already hosts the
association files. Given that, here are three options.

### Option A (minimal): per-link platform destinations via User-Agent sniff

Each link optionally carries a small set of destinations: an iOS target, an Android
target, and a default web target. On the hot path, quark reads the User-Agent, picks the
matching destination, and redirects. The iOS/Android targets can be an `https://` app
link (the OS opens the app if installed, else the store or web page the AASA/asset link
resolves to) or a direct store URL. The web target is the fallback for desktop, bots,
and anyone without the app.

- Hot-path cost: one User-Agent classification (cheap string check, or a small parser)
  plus the existing lookup. No network calls, no state. Negligible.
- Covers: installed-app open (handled by the OS via the files quark already hosts),
  store fallback, web fallback, desktop.
- Does not cover: deferred deep linking (land on the right screen after install).
- Good fit for an OSS redirect service. This is essentially what Firebase Dynamic Links
  did for the non-deferred case, minus the deferred matching.

### Option B (middle): Option A plus an interstitial for edge cases

Same as A, but for links flagged as "app links," quark serves a tiny HTML page that
attempts the custom URI scheme (or the universal link) via JavaScript and falls back to
the store after a timeout, and detects in-app browsers to show an "open in browser" hint.

- Hot-path cost: still cheap for plain links; the interstitial is an extra hop only for
  app-flagged links, and it is static HTML, no server state.
- Buys: slightly better behavior inside in-app browsers and a cleaner custom-scheme
  fallback on older setups.
- Costs: you now ship and maintain client-side redirect JavaScript, which is fiddly and
  breaks in odd ways across browsers. Marginal benefit over A for most users. Probably
  not worth it for v1.

### Option C (ambitious): SDK plus deferred matching

Build or integrate a mobile SDK, capture the Play Install Referrer on Android, run
best-effort matching on iOS, and store click state server-side so a fresh install can
ask quark for its deferred target on first launch.

- Hot-path cost: click resolution now has to persist click context (write path), and you
  add a first-launch resolution endpoint with its own storage and matching logic.
- This is rebuilding Branch / AppsFlyer. It requires an SDK that app developers embed,
  which is a fundamentally different product from a URL shortener. For an OSS redirect
  service with no app footprint, this is out of scope. It also inherits the iOS
  unreliability that even the paid vendors cannot fully solve post-ATT.

### Recommendation

Ship **Option A**. It matches quark's shape (stateless, cheap hot path, config per link),
uses the association files quark already hosts, and delivers the feature users actually
ask for: tap a short link, open the app if it is installed, otherwise go to the store or
a web page, with the right behavior per platform. Skip deferred deep linking in v1: it
needs an in-app SDK quark cannot provide, and on iOS it is unreliable even for the
vendors who specialize in it. Revisit only if quark ever grows a companion SDK, which
would be a separate product decision. Keep Option B's interstitial in mind as a later,
opt-in add-on for the in-app-browser problem, not as core.

---

## 6. Open questions for the owner

Answer these before implementation starts:

1. **Per-link config shape.** What does a link's app config look like? Minimum is three
   optional fields: iOS destination, Android destination, web fallback. Do you also want
   separate "app installed link" vs "store URL" fields, or collapse them into one
   `https://` app-link URL per platform and let the OS decide? Recommendation: one app
   link URL plus one store URL per platform, plus a web fallback, all optional.

2. **Which platforms.** iOS and Android only, or also a distinct desktop destination?
   Any need for per-OS-version or per-region routing, or is UA platform detection enough?

3. **Fallback behavior.** When a platform destination is missing for a link, fall back to
   the web URL (recommended) or return an error? When the web URL is also missing, what
   is the default?

4. **In-app browser handling.** Accept that links inside Instagram/TikTok/etc. land on
   the web fallback (Option A), or invest in an interstitial that nudges "open in
   browser" (Option B)? Recommendation: accept it for v1.

5. **Deferred deep linking.** Confirm it is out of scope for v1. If it is ever wanted,
   it changes quark from a stateless redirector into something that persists click state
   and ships an SDK, which is a much larger commitment.

6. **UA parsing cost and maintenance.** How much User-Agent detection is acceptable on
   the hot path? A coarse iOS / Android / other classification is a cheap substring
   check and needs almost no upkeep. A full UA-parsing library is heavier and needs
   updates. Recommendation: coarse classification, no external UA database.
