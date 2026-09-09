# Notifications (optional)

Off by default; this used to be the whole app. Turn it on with `notificationIndication=on`.

`GET /notifications` accepts only classic OAuth-app tokens, so this needs its own Client ID that you
register:

1. [github.com/settings/developers](https://github.com/settings/developers) → **New OAuth App**
2. Any **Homepage URL** (e.g. `http://localhost`), **Callback URL** blank
3. Create it, then click **Enable Device Flow**
4. Copy the **Client ID**

On the next start the app writes `client_id.txt`, opens it in your editor and waits for you to paste the
ID in. Then a device code, same as PR status. The token lands in `access_token.txt`, owner-readable only.

> **Known limitation:** unlike PR status, this half still runs at **startup** and blocks until you have
> finished. If that is inconvenient, leave `notificationIndication=off`.
