# Apple Developer ID Code Signing — Navod

Tento navod popisuje cely proces od zalozeni Apple Developer uctu az po pridani signing secrets do GitHub CI.
Po dokonceni bude macOS aplikace podepsana a notarizovana — uzivatelé ji spusti bez varovani GateKeeperu.

---

## Co potrebujes pred zacatkem

- Mac (nutne pro generovani CSR a export .p12)
- Kreditni karta (Apple Developer Program stoji $99/rok)
- Pristup ke GitHub repo (Settings → Secrets)

---

## Krok 0 — Zaloz Apple Developer Program ucet

Pokud jeste Apple Developer Program nemas:

1. Jdi na [developer.apple.com/programs/enroll](https://developer.apple.com/programs/enroll/)
2. Prihlas se svym Apple ID (nebo si zaloz novy na [appleid.apple.com](https://appleid.apple.com))
3. Vyber **Enroll as an Individual** (pro jednotlivce) nebo **Organization** (pro firmu)
   - Individual = staci, nevyzaduje zadne dokumenty
   - Organization = vyzaduje D-U-N-S cislo firmy (proces trva dele)
4. Vyplň osobni udaje, souhlas s podmínkami
5. Zaplat $99/rok kreditni kartou
6. **Schvaleni trva 1–3 dny** (Apple posilá potvrzovaci email)

> Po schvaleni dostanes email "Your Apple Developer Program membership is now active." Teprve pak pokracuj kroki nize.

---

## Krok 1 — Zjisti Apple Team ID

1. Jdi na [developer.apple.com/account](https://developer.apple.com/account) a prihlas se
2. V levem menu klikni na **Membership Details**
3. **Team ID** je 10-znakovy alfanumericky retezec (napr. `R9QY8Q23A2`)

Tento retezec pouzij jako hodnotu GitHub secretu `APPLE_TEAM_ID`.

---

## Krok 2 — Vygeneruj CSR (Certificate Signing Request) na Macu

CSR je soubor ktery rika Apple serveru jakou signovaci identitu chces vytvorit.

1. Otevri **Keychain Access** (Spotlight: `Keychain Access`)
2. V menu nahore: **Keychain Access → Certificate Assistant → Request a Certificate From a Certificate Authority...**
3. Vyplň:
   - **User Email Address**: tvuj Apple ID email
   - **Common Name**: cokoliv popisne (napr. `Messenger X Signing`)
   - **CA Email Address**: nechej prazdne
   - Vyber: **Saved to disk**
4. Klikni **Continue** → uloz soubor `CertificateSigningRequest.certSigningRequest` na plochu

---

## Krok 3 — Vygeneruj Developer ID Application certifikat

1. Jdi na [developer.apple.com/account/resources/certificates/list](https://developer.apple.com/account/resources/certificates/list)
2. Klikni na **`+`** (modre tlacitko vpravo nahore)
3. V sekci **Software** vyber **Developer ID Application**
4. Klikni **Continue**
5. **Profile Type**: vyber **G2 Sub-CA (Xcode 11.4.1 or later)** — nevybirej "Previous Sub-CA"
6. Klikni **Continue**
7. Klikni **Choose File** → vyber `CertificateSigningRequest.certSigningRequest` z Kroku 2
8. Klikni **Continue** → **Download**
9. Stahne se soubor `developerID_application.cer`

---

## Krok 4 — Pridej certifikat do Keychain

> **Pozor**: neklikej na soubor dvakrat — to se pokusi pridat certifikat do System Roots (read-only) a zobrazi chybu "The System Roots keychain cannot be modified". Pouzij drag & drop.

1. Otevri **Keychain Access**
2. V levem panelu klikni na **login** (pod "Default Keychains") — dulezite, musit byt vybran login
3. Otevri Finder → najdi stazeny `developerID_application.cer`
4. **Pretahni soubor mysi** primo do oblasti se seznamem polozek v Keychain Access
5. Certifikat se prida pod nazvem `Developer ID Application: Tvoje Jmeno (TEAMID)`

---

## Krok 5 — Exportuj jako .p12

.p12 je sifrovany soubor obsahujici certifikat + privatni klic — GitHub CI ho pouzije k podepisovani.

1. V Keychain Access vyhledej: `Developer ID Application`
2. Klikni **pravym tlacitkem** na **"Developer ID Application: Tvoje Jmeno (R9QY8Q23A2)"**
3. Vyber **Export "Developer ID Application: ..."**
4. Format: **Personal Information Exchange (.p12)** (melo by byt predvybrano)
5. Uloz jako napr. `messengerx-signing.p12` na plochu
6. Zadej **silne heslo** — toto bude hodnota secretu `APPLE_CERTIFICATE_PASSWORD`
   - Zadej stejne heslo do pole **Password** i **Verify**
   - > Pokud vidi chybu "Your password is not the same as the verify password" — hesla se lisi, zkus znovu
7. macOS te pozada o heslo k prehrihlasovacemu keychainu — zadej heslo sveho macOS uctu

---

## Krok 6 — Base64 enkoduj .p12

CI nema pristup k souborum — certifikat se musi prenest jako text.

Otevri Terminal a spust:

```bash
base64 -i ~/Desktop/messengerx-signing.p12 | pbcopy
```

Obsah schranky = hodnota pro GitHub secret `APPLE_CERTIFICATE`.

---

## Krok 7 — Ziskej App-Specific Password pro notarizaci

Notarizace = Apple server overi ze binary neobsahuje malware. Potrebuje tvuj Apple ID pro autentizaci, ale nechces pouzit hlavni heslo → app-specific password.

1. Jdi na [appleid.apple.com](https://appleid.apple.com) → prihlas se
2. **Sign-In and Security** → **App-Specific Passwords**
3. Klikni **+** nebo **Generate an app-specific password**
4. Jmeno: napr. `Messenger X Notarization`
5. Zkopiruj vygenerovane heslo (format: `xxxx-xxxx-xxxx-xxxx`) — toto bude `APPLE_PASSWORD`

> Heslo se zobrazi jen jednou — zkopiruj ho hned.

---

## Krok 8 — Ziskej presny retezec pro APPLE_SIGNING_IDENTITY

Po Kroku 4 spust v Terminalu:

```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```

Vypise neco jako:
```
1) ABCDEF1234567890... "Developer ID Application: Jan Novak (R9QY8Q23A2)"
```

Retezec v uvozovkach (vcetne uvozovek vynech) = hodnota `APPLE_SIGNING_IDENTITY`.

---

## Krok 9 — Pridej vsechno do GitHub

GitHub repo → **Settings → Secrets and variables → Actions → New repository secret**

| Secret | Kde ho ziskas |
|--------|---------------|
| `APPLE_TEAM_ID` | Krok 1 — napr. `R9QY8Q23A2` |
| `APPLE_ID` | Tvuj Apple ID email |
| `APPLE_SIGNING_IDENTITY` | Krok 8 — napr. `Developer ID Application: Jan Novak (R9QY8Q23A2)` |
| `APPLE_CERTIFICATE` | Krok 6 — base64 obsah ze schranky |
| `APPLE_CERTIFICATE_PASSWORD` | Krok 5 — heslo ktere jsi zadal pri exportu .p12 |
| `APPLE_PASSWORD` | Krok 7 — app-specific password z appleid.apple.com |

Po pridani vsech 6 secrets bude dalsi CI release automaticky podepsan a notarizovan. Zadne zmeny v kodu nejsou potreba — CI je uz plne zadrátovano.

---

## Overeni ze vse funguje

Po pridani secrets spust release workflow (nebo push tag). V CI logu hledej:

```
Signing application...
Notarizing application...
Stapling notarization...
```

Pokud notarizace selze, nejcastejsi priciny:
- Spatny `APPLE_ID` nebo `APPLE_PASSWORD` → zkus vygenerovat novy app-specific password
- Spatny `APPLE_SIGNING_IDENTITY` → over retezec prikaze z Kroku 8 (case-sensitive)
- .p12 nema privatni klic → v Keychain Access musi byt certifikat se sipkou rozbalovaci (obsahuje private key); pokud nema, vygeneruj novy CSR na stejnem Macu kde je private klic

---

## Bezne chyby a reseni

| Chyba | Reseni |
|-------|--------|
| "The System Roots keychain cannot be modified" | Nepouzivej dvojklik na .cer; pouzij drag & drop do login keychainu (Krok 4) |
| "Your password is not the same as the verify password" | Zadej stejne heslo do obou poli (Krok 5) |
| Certifikat nema sipku (expand) v Keychain | Private klic chybi — vygeneruj CSR na stejnem Macu, ne na jinem |
| Notarizace selze s "Your Apple ID account does not have..." | Ucet musi mit Apple Developer Program — ne jen bezny Apple ID |
