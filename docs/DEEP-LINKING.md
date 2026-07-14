**English** · [Português](DEEP-LINKING.PT_BR.md)

# Deep linking: hosting the app-association files

For a short link to open a native mobile app instead of the browser, the domain
that serves the redirect has to prove it is allowed to speak for that app. iOS
and Android both do this by fetching a small JSON file from the domain and
checking it against the app installed on the device. quark hosts those two
files so your links can become iOS Universal Links and Android App Links.

This page covers hosting the files, which is the piece quark ships today.
Opening the app on a per-request basis (device-aware redirect) is a separate
follow-up, see the note at the end.

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

## Not yet: device-aware redirect

Device-aware redirect, actually opening the app when a link is tapped (detecting
iOS or Android and sending the device to an app URI or the store, with a web
fallback), is a deferred follow-up and is not yet implemented. Hosting the
association files is the prerequisite that work builds on. It needs product
decisions (which platforms, per-link app scheme, fallback behavior) and overlaps
the redirect-rules work, so it is left for a later, interactive round. Today
quark hosts the files, which is what lets the OS associate your domain with the
app in the first place.
