# E2E-тесты (`tests_e2e/`)

Контур сквозных (E2E) смоук-тестов поверх штатного CLI `kuyase run --headless --board <board> --snapshot --exec "<cmd>"` без самописных TCP/ADB-клиентов:

1. **`test_01_yam.py`** — проверка оригинальных установщиков [`nyakokitsu/yam`](https://github.com/nyakokitsu/yam) (`install.sh` и `install.ps1`): автоопределение модели по `DEVICE_IDENTITY` с учётом SoC (`amlogic,axg` vs `amlogic,a5`), прогон `install.sh` и проверка `/usr/bin/yam list` и `/usr/bin/yam doctor`.
2. **`test_02_luna.py`** — проверка официальных установщиков рантайма [`nyakokitsu/luna`](https://github.com/nyakokitsu/luna) (`install.sh` и `install.ps1`), старта демона на `:8099`, `/api/device`, управления световым кольцом (`/api/led/ring`), сегментными часами (`/api/clock`) и громкостью (`/api/volume/set`).
3. **`test_03_airplay.py`** — смоук-тест плагина [`ryysel/Station-airplay`](https://github.com/ryysel/Station-airplay) (v2.0+) поверх `yam` и `luna`: установка релизного пакета `.yampkg`, ожидание `/run/station-airplay/private-bus` и RTSP `:5000`/`:7000`, проверка `org.gnome.ShairportSync.GetControlStatus`.
4. **`test_04_quasar_q.py`** — сквозная проверка штатной утилиты `/system/vendor/quasar/q` (`version`, `device-id`, `full-info`, `wifi-info`, `getprop`, `glagol-connections`, `stop`, `start`, `restart`, `backend-dev`, `backend-prod`), интерактивного сеанса `q cli` (`:9875`) и сервиса `glagold` (`:9899` и `:1961`).

>[!note]
>Все тесты выполняются через `kuyase run --headless --board <board> --snapshot --exec "<cmd>"`. По завершении `--exec` сам `kuyase` проверяет код возврата гостевого скрипта и отсутствие ошибок в `BridgeStateSnapshot` (`ipc_error_count == 0`, `led_trigger_conflict_count == 0`).

## Запуск

```bash
# Запустить все E2E-тесты на одной плате (например, station2):
KUYASE_E2E_BOARD=station2 python -m unittest discover -s tests_e2e -v

# Запустить только смоук-тест установщика YAM:
python -m unittest tests_e2e/test_01_yam.py -v

# Запустить смоук-тест Station-airplay:
KUYASE_E2E_BOARD=station2 python -m unittest tests_e2e/test_03_airplay.py -v
```

## Переменные окружения

| Переменная | Назначение |
| :--- | :--- |
| `KUYASE_E2E_BOARD` | Профиль платы (например, `station2`, `cucumber`, `orion`) или список через запятую. По умолчанию обходит все `work/*-system.ext4`. |
| `KUYASE_BIN` | Явный путь к исполняемому файлу `kuyase` или `kuyase-headless` (по умолчанию ищет `kuyase[.exe]` и `kuyase-headless[.exe]` в `target/release/`, `target-headless/release/`, `Releases/` и корне репозитория). |
| `YAM_REPO_DIR` | Путь к локальной рабочей копии репозитория `yam` (по умолчанию `../yam`, если каталог существует, иначе берётся из `nyakokitsu/yam`). |
| `KUYASE_AIRPLAY_PKG` | Путь к локальному пакету `station-airplay-*.yampkg` для `test_03_airplay.py` (по умолчанию скачивается из `ryysel/Station-airplay` Releases). |
