# Broker sign-in

OpenAlgo Desktop keeps **one set of keys per broker**, for as many brokers as
you like, and **one broker is logged in at a time**. Two pages handle it:

- **Broker Configuration** (`/broker/config`) is where keys are saved. Open it
  from the Select Broker page, or from the **Broker** tab in Profile.
- **Select Broker** (`/broker`) shows a card per broker, with a badge
  (**Active** for the broker logged in now, **Configured** for one with saved
  keys, **Not Configured** otherwise) and the exchanges it trades. Brokers
  with saved keys are listed first, and the search box filters the cards.

Every broker is set up the same way:

1. Create an API app on your broker's developer portal. If the broker signs
   in on its own website (see below), the portal asks for a redirect URL:
   enter `http://127.0.0.1:5000/<broker>/callback` (for example
   `http://127.0.0.1:5000/upstox/callback`). If you changed the app port in
   Server Settings, use that port instead of 5000. The Broker Configuration
   page shows the exact address to register.
2. In OpenAlgo Desktop open **Broker Configuration** and choose your broker.
   The page shows only the keys that broker needs, labelled the way the
   broker names them (for example **App ID** for Fyers, **SmartAPI Key** for
   Angel One, **UCC** and **Access Token** for Kotak Securities), with a hint
   for each and a short note on how that broker's login works. Enter the keys
   and choose **Save**.
3. Open **Select Broker** and choose **Login with** on your broker's card.

**Saved keys are never shown again in full.** Once saved, the page shows a
masked value such as `Saved: abc***xyz` (only `***` for a short value), and
the input below it is empty: leave it blank to keep the saved value, or type
a new value to replace it. Choosing another broker in the list clears
whatever you typed, so nothing typed for one broker is saved under another.

**Redirect URL.** For brokers that sign in on their own website, the page
fills in the redirect URL automatically: this app's address, or your Host
Server URL while the tunnel (ngrok) is on. You can type another address; it
must be a full address ending in `/<broker>/callback`. **Use automatic**
goes back to the automatic address (an empty field means automatic too).
The redirect URL is kept with that broker's keys.

**New keys for the broker that is logged in** end its session, because a
session belongs to the keys it signed in with. OpenAlgo asks first, in the
same words as a switch ("Saving new keys for X ends your X session. Open
positions and orders stay with X."), then saves the keys and ends the
session; log in again to use them. Changing only the redirect URL ends
nothing.

**Forget this broker** removes a broker's saved keys from this computer,
after you confirm. Forgetting the broker that is logged in signs you out of
it first; your open positions and orders stay at the broker.

**One account per broker.** OpenAlgo keeps one key set per broker, by
design: there is no list of several accounts at the same broker. To use
another account at a broker, enter that account's keys in place of the
saved ones (or choose **Forget this broker** and add them again); the next
login is then checked against the new account.

What happens when you choose **Login with** depends on how your broker signs
in. Brokers fall into three groups.

## Brokers that sign in on their own website

Zerodha, Fyers, Upstox, Dhan, Arrow, Paytm Money, Pocketful, HDFC Sky, HDFC
Securities, IIFL Capital, Alice Blue, CompositEdge, RMoney, Flattrade,
Shoonya, Zebu and TradeSmart.

**Login with** takes you to the broker's login page. Log in there, approve
access, and the broker sends you back to OpenAlgo, which finishes the sign-in
and opens the dashboard. If the sign-in fails, you come back to Select Broker
with the reason shown on that broker's card.

- The redirect URL in your broker app must match the one on the Broker
  Configuration page exactly. Upstox in particular refuses a sign-in whose
  redirect URL differs by a single character, including `localhost` in place
  of `127.0.0.1`.
- Always start from **Login with** in OpenAlgo. A sign-in that OpenAlgo did
  not start, or one that has expired, is refused with "This broker sign-in
  was not started from OpenAlgo or has expired". Choose Login with again.
- Finish the broker's login within a few minutes of starting it. Some brokers
  (Dhan, Flattrade, Shoonya, Zebu, TradeSmart, Arrow, HDFC Sky, HDFC
  Securities and Alice Blue) do not send back OpenAlgo's sign-in reference,
  so for them OpenAlgo accepts only the most recent sign-in started from the
  same window, within three minutes.
- Zerodha needs an active Kite Connect subscription, and Dhan an active Data
  API subscription, for market data.

## Brokers bound to your client ID

For some of the brokers above OpenAlgo checks that the account you log in with
is the account you set up, so that nobody can slip their own session into your
app. These brokers need your client ID:

| Broker | Where the client ID goes |
| --- | --- |
| Dhan | In the first key, as `client_id:::api_key` |
| Flattrade | In the first key, as `client_id:::api_key` |
| Shoonya, Zebu, TradeSmart | In the first key, as `userid:::client_id` (your user ID, then the app's client ID or key) |
| Arrow, HDFC Sky, HDFC Securities | The **Client ID** field Broker Configuration shows for these brokers |
| Alice Blue | The **User ID** field Broker Configuration shows for it |

If the client ID is missing, the sign-in stops and names the field to fill in
on the Broker Configuration page. If you log in to the broker with a
different account, OpenAlgo refuses it and changes nothing: "This sign-in is
for a different account than the one set up in OpenAlgo". To switch to
another account at the same broker, log out of the broker, save the broker's
keys again with the new account's details, then log in.

## Brokers that sign in with a form in the app

**Login with** opens a small login card inside OpenAlgo with only the fields
your broker asks for, and a link back to Select Broker. Type them each time
you log in; OpenAlgo uses them for that sign-in only and does not save them.
If the broker refuses, the card shows the reason so you can try again with a
fresh code.

| Broker | What the form asks for |
| --- | --- |
| Angel One | Client ID, PIN, TOTP |
| Kotak Securities | Mobile number, TOTP from the Kotak NEO app, MPIN |
| 5 Paisa | Login email or client ID, PIN, TOTP |
| Firstock | User ID, password, TOTP |
| Motilal Oswal | User ID, password, date of birth, TOTP |
| mStock by Mirae Asset | Password, TOTP |
| Tradejini | CubePlus login PIN, TOTP |
| Nubra | The OTP that Nubra texts to your registered mobile when the form opens |
| IndMoney | MPIN and TOTP |
| Definedge | The OTP that Definedge sends to your registered mobile or email when the form opens |
| Groww | TOTP if your Groww API key uses TOTP, or an access token you paste; leave both empty to sign in with the saved API key and secret |

Notes:

- **TOTP** is the 6-digit code from your authenticator app for that broker
  account. Codes change every 30 seconds; if a sign-in fails, wait for the
  next code. Check that your computer's clock is set automatically.
- **5 Paisa**: the first key is entered as `User_Key:::User_ID:::client_id`,
  as the Broker Configuration page shows.
- **IndMoney**: the first key is the Client ID shown at indstocks.com, API
  Trading, Access Tokens. Leave the Access Token empty to log in with MPIN and
  TOTP.
- **Samco** signs in with the saved API key and secret on its own **Connect**
  page, which also checks that your computer's internet address matches the
  static IP registered with Samco.
- **Angel One, RMoney and Tradejini** need no API secret; Broker
  Configuration does not ask for one.

## Brokers that sign in with saved keys

The Symphony XTS brokers (5 Paisa (XTS), JainamXts, Ibulls, IIFL, Wisdom
Capital), Delta Exchange and Dhan (Sandbox) sign in with the keys you saved:
**Login with** signs you in with one click, with nothing to type. For the
XTS brokers (and CompositEdge and RMoney, which sign in on their own
website), also fill in the **Market Data App Key** and **Market Data Secret
Key** on the Broker Configuration page; they are the market data keys from
the same XTS account, and quotes and the live feed need them.

Delta Exchange is a crypto exchange: its symbols use the `CRYPTO` exchange.

## Signing in to OpenAlgo, and the daily broker sign-in

Open the app and sign in with your username and password (and the code from
your authenticator app, if you turned on two-factor sign-in). OpenAlgo then
tries to resume your broker session from the same trading day: if the broker
still accepts it, you go straight to the dashboard; otherwise you go to
Select Broker to log in. The master contract (the list of symbols) downloads
after a broker login; the **Master Contract** indicator on the dashboard
shows its progress.

Indian brokers end API sessions every night, so OpenAlgo ends the broker
session at about 03:00 IST, after which you log in again from Select Broker.
Logging out of OpenAlgo also ends the broker session and returns you to the
sign-in page.

## Switching brokers

Only one broker is logged in at a time. To switch, choose **Login with** on
another configured broker's card. While a broker is logged in, OpenAlgo asks
you to confirm first: "Signing in to Y ends your X session. Open positions
and orders stay with X." Confirming ends that session in OpenAlgo (stop-loss
and target exits OpenAlgo places for it stop, and your trading platforms can
no longer trade through it) before the new sign-in starts. OpenAlgo enforces
this itself: a sign-in that completes always ends any other broker's session
first, even if two sign-ins finish at the same moment.

Your keys for the broker you leave stay saved, so switching back needs only a
login, not your keys again. No restart is needed.

Sign-in failing? See [Troubleshooting](troubleshooting.md#broker-sign-in-fails).
