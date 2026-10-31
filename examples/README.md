# Пример Headless-тестирования

Демо-скрипт [`test_yam_lineup.py`](./test_yam_lineup.py) тестирует автоопределение модели колонки (`detect_platform`) в установщике пакетного менеджера для Яндекс Станций [`nyakokitsu/yam`](https://github.com/nyakokitsu/yam/blob/caf835a4046d24810cc40d75fb823e4d4dd23de8/install.sh) сразу по всем собранным в `work/` образам (`*-system.ext4`) через `kuyase run --headless --board all --snapshot --exec "..."`.

Скрипт скачивает `install.sh` по закреплённому хешу коммита (`caf835a4046d24810cc40d75fb823e4d4dd23de8`), без модификаций вырезает из него функции `device_identity()` и `detect_platform()`, запускает их внутри каждой виртуальной колонки и сверяет результат с эталонным полем `"deviceType"` из заводского конфига прошивки `/system/vendor/quasar/quasar.cfg`.

## Легенда вывода

* `compatible` — строка из `/proc/device-tree/compatible`, которую читает `device_identity()` в `install.sh`;
* `quasar.cfg` — эталонный `"deviceType"` прошивки из `/system/vendor/quasar/quasar.cfg`;
* `yam_profile` — профиль, который вернула функция `detect_platform()` из `install.sh`.

## Запуск

```bash
# Проверить все образы в папке work/ (на закреплённом коммите caf835a404):
python examples/test_yam_lineup.py

# Проверить только конкретную модель:
python examples/test_yam_lineup.py --board plumnc

# Проверить против актуальной ветки main репозитория YAM:
python examples/test_yam_lineup.py --ref main
```
