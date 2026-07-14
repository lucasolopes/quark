**English** · [Português](DEEP-LINKING.PT_BR.md)

# Deep linking: hosting the app-association files

For a short link to open a native mobile app instead of the browser, the domain
that serves the redirect has to prove it is allowed to speak for that app. iOS
and Android both do this by fetching a small JSON file from the domain and
checking it against the app installed on the device. quark hosts those two
files so your links can become iOS Universal Links and Android App Links.

This page covers two things: hosting the files, which is what the OS needs
before it will tie your domain to an app, and the device-aware redirect that
decides where a click goes once it reaches quark. The redirect section is at the
end.

## The two files

**iOS: `apple-app-site-association` (AASA).** A JSON document that lists the app
IDs allowed to handle links on this domain and which URL paths each app claims.
When someone taps a link to your domain, iOS looks for this file, and if the
domain and the installed app agree, the link opens in the app instead of
Safari.

**Android: `assetlinks.json` (Digital Asset Links).** A JSON document that
states which Android app (package name plus signing certificate fingerprint) is
allowed to handle links for this domain. Android checks it the same way before
opening a verified App Link in the app.

Neither platform will associate a domain with an app unless it can fetch the
matching file from the domain first. That is why hosting these files is the
prerequisite for any app-opening behavior. Without them the OS refuses the
association and every link just opens the browser.

## How the OS fetches them

The OS requests the files directly, anonymously, over HTTPS. quark serves them
at the exact paths each platform looks for:

| File | Path quark serves |
| --- | --- |
| AASA (iOS) | `/.well-known/apple-app-site-association` |
| AASA (iOS, legacy) | `/apple-app-site-association` |
| assetlinks.json (Android) | `/.well-known/assetlinks.json` |

Rules the OS enforces, and that quark follows:

- **`Content-Type: application/json`.** Both files are served with this type.
  AASA has no `.json` extension but is still JSON.
- **HTTPS, no redirect.** The OS fetches over HTTPS and will not follow a
  redirect for these paths. quark serves the file at the path directly, on the
  same domain that serves your redirects, so put quark behind TLS (a
  reverse proxy or CDN terminating HTTPS, as in the deploy guide).
- **No auth.** The fetch is anonymous, so these three GET routes are public.
  Writing the files is admin-only (see below).
- **404 when unset.** If you have not stored a file, quark returns 404 rather
  than an empty JSON body. That is what the OS expects from a domain that hosts
  no association.

The legacy root path `/apple-app-site-association` exists because some older iOS
versions probe the domain root before the `.well-known` path. quark serves the
same AASA document at both.

## How to produce the files

quark does not generate these files or invent their fields. The exact contents
come from your mobile team, because they depend on the app's ID, package name,
and signing certificate. Apple and Google change the format over time (AASA
moved from `paths` to `components`, for example), so the authoritative field
reference is their docs, not this page:

- Apple, "Supporting associated domains":
  https://developer.apple.com/documentation/xcode/supporting-associated-domains
- Google, "Verify Android App Links":
  https://developer.android.com/training/app-links/verify-android-applinks

Xcode and Android Studio emit these files as part of the app build. Ask the
mobile team for the current `apple-app-site-association` and `assetlinks.json`,
then paste them into quark as-is. quark validates only that the body is valid
JSON and within a size cap (64 KiB). It does not check the app IDs or
fingerprints, only the OS and the app can judge whether the association is
correct.

## Setting them in the panel

Open the **App Links** page in the admin panel. There are two editors, one for
each file:

1. Paste the JSON your mobile team gave you into the matching editor.
2. If the JSON is invalid, the editor flags it and Save stays disabled. Fix the
   paste until it parses.
3. Click **Save** to store and start serving the file.
4. Click **Clear** to remove a stored file (the path goes back to 404).

After Save, the file is live at its well-known path immediately. You can confirm
with a request, for example `curl https://your-domain/.well-known/assetlinks.json`,
and check the body and the `application/json` content type.

## Device-aware redirect

A link can carry two optional destinations on top of its normal URL, one for iOS
and one for Android. When a click comes from that platform and the app is not
installed, so the tap is not caught by the OS and reaches quark, quark sends the
click to the platform destination instead of the normal URL. If the app is
installed, the OS opens it through the association file and quark never sees the
request.

quark picks the destination from the click's platform:

| Click platform | Redirects to |
| --- | --- |
| iOS, and the link sets an iOS destination | the iOS destination |
| Android, and the link sets an Android destination | the Android destination |
| desktop, other, or the platform has no destination set | the normal link URL (fallback) |

The fallback never fails. A click from desktop, or from a platform whose
destination is not set, goes to the link's normal URL, the same as a link that
uses no app destinations at all.

Only links that set an app destination pay for this. quark checks whether the
link has an iOS or Android destination before it looks at the User-Agent. A link
with neither takes the plain redirect with no extra work, so the common hot path
is unchanged. Platform detection is a substring check on the User-Agent (iPhone,
iPad, or iPod for iOS, Android for Android), not a parsing library.

The app destinations are validated the same way as the main URL. Each one has to
be http or https and pass the SSRF guard that blocks internal and private hosts,
so an app destination cannot point quark at an internal address the main URL
could not reach either.

### Limits

This version does the redirect only. Two things it does not do:

- **Deferred deep linking.** Sending a user who does not have the app to the
  store, then opening the app on the right screen after they install it, is not
  handled. Closing that loop needs an SDK embedded in the mobile app to read the
  pending link on first launch, and quark does not ship a mobile SDK.
- **In-app-browser routing.** Clicks opened inside an app's own webview
  (Instagram, TikTok, and the like) are not detected or steered out of the
  webview. They fall through to the normal behavior.

Both build on the association files from the hosting sections above, which are
the prerequisite for any of this. Without them the OS will not hand a link to the
app in the first place.
