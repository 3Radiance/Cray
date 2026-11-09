# Вклад в разработку Cray

Спасибо за интерес к проекту! 🎉

[Read in English ->](./CONTRIBUTING.md)

## С чего начать

1. Сделайте форк репозитория
2. Создайте ветку для вашей фичи или исправления:
   ```bash
   git checkout -b feature/my-layer
   ```
3. Внесите изменения и закоммитьте их
4. Запушьте ветку и откройте Pull Request

## Среда разработки

Для сборки требуются Rust, `cmake` и компилятор C/C++ (TLS собран на
BoringSSL через `btls`).

```bash
cargo build --release
cargo test --workspace
```

Перед пушем:

```bash
cargo fmt --all
cargo clippy --workspace
cargo test --workspace
```

CI прогоняет ровно эти три проверки при каждом пуше.

## Как добавить слой

Хотите добавить слой — вам сразу в
[docs/ru/writing-layers.md](docs/ru/writing-layers.md) (английская версия:
[docs/eng/writing-layers.md](docs/eng/writing-layers.md)). Там разобраны
контракт слоя, полный разбор настоящего слоя на реальном коде, `Ports`,
`Cx`, таймауты, логирование и тестирование.

Новый слой приходит в комплекте из:

- кода, зарегистрированного в `layers/src/lib.rs`;
- тестов в `layers/tests/`;
- страницы в `docs/ru/` и точно такой же страницы в `docs/eng/`;
- обновлённых `README.md` и `README.ru.md`, если изменение затрагивает
  списки возможностей.

## Формат коммитов

Мы следуем Conventional Commits: строчными, со scope по крейту или слою.

```
<type>(<scope>): <короткая суть>
```

Типы: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `ci`, `chore`.

Реальные примеры из истории:

- `feat(xhttp2): add packet-up mode with seq reordering`
- `fix(auth): fix allocations`
- `test(reorder): add new unit tests for reordering logic`
- `refactor(tls): SNI cert selection`
- `docs: upd docs`

Scope — это крейт или слой, к которому относится изменение (`xhttp2`,
`vless`, `tls`, `dial`…). В докоммитах scope можно опускать. В старой
истории встречается префикс `CI:` с большой буквы — правильная форма
сейчас строчная: `ci:`.

## Рекомендации по PR

- Держите PR сфокусированными: одна фича или исправление на PR
- Добавляйте или обновляйте тесты, где это имеет смысл
- Обновляйте документацию, если ваше изменение её касается
- Запускайте `cargo fmt --all` и `cargo clippy --workspace` перед пушем

## Лицензия

Внося вклад, вы соглашаетесь с тем, что ваш вклад будет лицензирован под
лицензией [GPL-3.0-or-later](./LICENSE).

Список участников — в [CONTRIBUTORS.md](./CONTRIBUTORS.md).
