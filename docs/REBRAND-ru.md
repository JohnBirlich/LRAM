# Переименование в LRAM и отвязка от исходного проекта

Сборка больше не называется RoALT и не ходит на GitHub автора ни за обновлениями, ни за
донатами, ни в его Discord. Здесь — что именно поменялось, что осталось как было (и почему),
и что нужно сделать, чтобы выпустить свою версию.

## Что теперь называется LRAM

| Место | Было | Стало |
|---|---|---|
| Имя продукта (`package.json`, `tauri.conf.json`, окно, заголовок страницы) | RoALT | LRAM |
| Идентификатор приложения | `com.inspectsoftware.roalt` | `com.lram.manager` |
| Издатель в установщике | inspectsoftware | JohnBirlich |
| Имя установщика | `RoALT-Setup-x.y.z.exe` | `LRAM-Setup-x.y.z.exe` |
| Папка программы | `%LOCALAPPDATA%\Programs\RoALT` | `…\LRAM`, файл `LRAM.exe` |
| Папка данных | `%LOCALAPPDATA%\RoALT` | `%LOCALAPPDATA%\LRAM` |
| Файл журнала | `roalt.log` | `lram.log` |
| AppId установщика | `{F65209C5-…}` | новый `{E647C13A-…}` (ставится рядом с исходным) |
| Текст интерфейса и журналов | «RoALT» | «LRAM» (только английский) |
| Ссылки в интерфейсе, README, allowlist в `capabilities` | `inspectsoftware/RoALT` | `JohnBirlich/RAM` |

Кнопка «Поддержать» (донат на игровые пропуска автора) удалена из сайдбара, компонент
`src/features/donate` — тоже. Кнопка Discord в rich presence удалена.

## Что намеренно оставлено старым

Эти строки выглядят как «недоделанное переименование», но менять их нельзя без потери данных:

- **`b"RoALT/db/v1"` и `b"RoALT/vault/v1"`** — энтропия DPAPI (`storage/cipher.rs`,
  `vault/dpapi.rs`). DPAPI подмешивает её в ключ: смена строки = невозможность расшифровать
  существующие базу и хранилище.
- **AAD `roalt.*`** (`roalt.cookie:`, `roalt.vault.key`, `roalt.discord.webhook`,
  `roalt.favourite`) — те же причины.
- **Имя крейта `roalt`** и бинарника `roalt.exe` — видно только разработчику; установщик
  переименовывает файл в `LRAM.exe`.
- **Магия резервных копий `ROALTBK1`** — старые бэкапы должны открываться.

Имя файла базы переименовано (`roalt.db` → `lram.db`): на старте `adopt_legacy_names`
переносит `roalt.db`, `roalt.db-wal`, `roalt.db-shm` и `roalt.db.key` на новые имена, если
новых файлов ещё нет. Покрыто тестом `old_database_names_are_taken_over_once`.

Чтобы данные прежней сборки не потерялись, `default_dir()` берёт `%LOCALAPPDATA%\RoALT`,
если папки `LRAM` ещё нет, а в старой лежит база. Перенести данные в путь с новым именем
можно кнопкой «Сменить папку данных» в настройках (перенос там безопасный, с проверками).

## Обновления

Встроенный автообновлятор **выключен**: в `self_update.rs` пустой `PUBLIC_KEY`, поэтому
проверка версий ничего не запрашивает, `spawn()` сразу выходит, а `self_update` по умолчанию
`false`, и переключателя в настройках больше нет. Установка новых версий — вручную с
GitHub Releases.

Если однажды понадобится вернуть:

1. `node scripts/update-key.mjs` — печатает пару ключей: публичную половину в
   `const PUBLIC_KEY: &str = "…";` в `src-tauri/src/self_update.rs` и приватную (PKCS#8 PEM)
   в секрет `LRAM_UPDATE_KEY` репозитория.
2. Вернуть в настройках переключатель (`SelfUpdateRow` в `SettingsView.tsx`) и
   `self_update: true` в `models.rs`.
3. Выпускать релизы через `.github/workflows/release.yml`: без секрета он собирает установщик
   без `.sig` (шаг подписи пропускается), с секретом — подписывает и прикладывает подпись.

`LATEST` в `self_update.rs` и `RELEASES`/`DOWNLOAD` в интерфейсе уже указывают на
`JohnBirlich/RAM`.

## Что сделано и что осталось

- **Логотип и иконки** — свои: `src-tauri/icons/app-icon.svg` (вектор) и
  `scripts/make-icons.py` (Pillow собирает png и icon.ico из того же описания). Тот же знак в
  `Logo` (`src/ui/Icon.tsx`), в сплэше и в `docs/images/banner*.png`.
- **Скриншоты** — сняты со старого вида и удалены, чтобы не показывать неправду. Новые снимаются
  с работающего приложения (`Win+Shift+S` или любой скриншотер) и кладутся в `docs/images/`.
- **Discord rich presence** выключен: `CLIENT_ID` пуст. Своё приложение регистрируется на
  https://discord.com/developers/applications (Rich Presence → Assets: картинка с ключом
  `lram`), а его id передаётся при сборке:
  `$env:LRAM_DISCORD_APP_ID="1234567890"; powershell -File scripts\build-installer.ps1`.
- **Подпись SignPath** — не сделана: нужен аккаунт и одобрение проекта, из песочницы это
  невозможно. Workflow уже готов: см. ниже.

## SignPath: что нужно от вас

1. Регистрация на https://signpath.io и заявка на бесплатную лицензию для open source
   (проект `LRAM`, репозиторий `JohnBirlich/RAM`).
2. В SignPath: организация → проект `LRAM` → signing policy `release-signing` →
   artifact configuration для Inno Setup (`github-ci`).
3. В GitHub: секрет `SIGNPATH_API_TOKEN` и переменная `SIGNPATH_ORGANIZATION_ID`
   (Settings → Secrets and variables → Actions). Больше ничего менять не нужно:
   `release.yml` сам загрузит неподписанный установщик артефактом, отправит его на подпись,
   заберёт подписанный и только потом подпишет его ключом автообновления.

## Выпуск своей сборки

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1
```

Версия берётся из `tauri.conf.json` и должна совпадать в `package.json` и
`src-tauri/Cargo.toml` (сейчас везде `0.10.0`). Установщик ляжет в
`dist-installer\LRAM-Setup-0.10.0.exe`. Тег `v0.10.0` запускает workflow, который соберёт
черновик релиза.
