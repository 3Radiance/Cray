# Вклад в разработку Cray

Спасибо за интерес к проекту! 🎉

## Начало работы

1. Сделайте форк репозитория
2. Создайте ветку для вашей фичи или исправления:
   ```bash
   git checkout -b feature/my-layer
   ```
3. Внесите изменения и закоммитьте их
4. Запушьте ветку и откройте Pull Request

## Среда разработки

Для сборки требуются Rust, `cmake` и компилятор C/C++ (TLS реализован на BoringSSL через `btls`).

```bash
cargo build --release
cargo test --workspace
```

## Стиль кода

- Форматирование обеспечивается `rustfmt` — запускайте `cargo fmt --all` перед коммитом
- Линтинг обеспечивается `clippy` — запускайте `cargo clippy --workspace`, чтобы предупреждения отсутствовали
- CI выполняет обе проверки при каждом пуше

---

## Написание пользовательского слоя

Слои — это строительные блоки Cray. Каждый слой выполняет одну задачу: принимает соединение, трансформирует поток или реализует протокол. Слои компонуемы — движок связывает их на основе вашей конфигурации.

### Анатомия слоя

Каждый слой состоит из **трёх частей**:

| Часть | Назначение |
|-------|------------|
| **Структура конфигурации** | Десериализует параметры TOML из файла конфигурации |
| **Структура слоя** | Хранит состояние выполнения; реализует трейт `Layer` |
| **Функция `build()`** | Фабрика, которая парсит конфигурацию и создаёт слой |

### Пошаговое создание слоя

Создадим простой слой `delay`, который добавляет фиксированную задержку перед передачей данных.

#### 1. Определите структуру конфигурации

```rust
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]  // Отклоняет неизвестные поля — ловит опечатки на ранней стадии
struct DelayCfg {
    millis: Option<u64>,        // Опциональное поле
}
```

Структура конфигурации напрямую соответствует TOML-таблице в вашем файле конфигурации:

```toml
[[outbound.layers]]
type = "delay"
millis = 100
```

#### 2. Определите структуру слоя

```rust
use cray_core::{BuildCx, Layer, Ports, Side};
use std::time::Duration;

pub struct Delay {
    millis: u64,
    side: Side,  // Сохраняется при сборке — определяет поведение в call()
}
```

Поле `side` **обязательно**. Оно сообщает слою, работает ли он как входящий или исходящий, чтобы `call()` мог направить к правильной логике.

#### 3. Реализуйте трейт `Layer`

```rust
use cray_core::{BoxFuture, BoxedStream, Cx, Input, Output};
use anyhow::Result;

impl Layer for Delay {
    // Что этот слой делает с потоком
    fn ports(&self) -> Ports {
        Ports::WRAP  // Принимает поток, отдаёт поток
    }

    // Объявите, что этому слою нужно из контекста
    fn requires(&self) -> Vec<cray_core::Key> {
        Vec::new()  // Этому слою ничего не нужно
    }

   // Объявите, что этот слой предоставляет другим слоям через Cx
    fn provides(&self) -> Vec<cray_core::Key> {
        Vec::new()  // Этот слой ничего не предоставляет
    }

    // Основная логика — вызывается, когда цепочка достигает этого слоя
    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            match self.side {
                Side::Inbound => {
                    // Входящая логика: принять/обработать входящее
                    let stream: BoxedStream = input.stream()?;
                    // ... трансформировать ...
                    Ok(Output::Stream(stream))
                }
                Side::Outbound => {
                    // Исходящая логика: подключиться/направить дальше
                    let stream: BoxedStream = input.stream()?;
                    // ... трансформировать ...
                    Ok(Output::Stream(stream))
                }
            }
        })
    }
}
```

#### Передача пользовательских данных через Cx

Контекст `Cx` не ограничен встроенными типами — вы можете передавать любую структуру `Send + Sync` между слоями. Это позволяет связывать слои, которые ничего не знают друг о друге, напрямую.

**Пример**: слой рукопожатия создаёт ключи сессии, слой шифрования их потребляет:

```rust
// Определите вашу общую структуру
pub struct SessionKeys {
    pub send_key: [u8; 32],
    pub recv_key: [u8; 32],
}

// Слой-производитель — сохраняет ключи в Cx
impl Layer for Handshake {
    fn provides(&self) -> Vec<Key> {
        vec![key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream = input.stream()?;
            let keys = do_handshake(stream).await?;
            cx.put(keys);  // Теперь доступно для последующих слоёв
            Ok(Output::Stream(stream))
        })
    }
}

// Слой-потребитель — читает ключи из Cx
impl Layer for Crypto {
    fn requires(&self) -> Vec<Key> {
        vec![key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let keys = cx.get::<SessionKeys>()
                .ok_or_else(|| anyhow!("нет SessionKeys в Cx"))?;
            // ... использовать ключи ...
        })
    }
}
```

Нет ограничений на то, что вы можете хранить — любая структура, перечисление или коллекция работает, если она `Send + Sync + 'static`. Движок проверяет при запуске, что все `requires()` удовлетворяются каким-либо `provides()` ранее в цепочке.

#### 4. Напишите функцию `build()`

```rust
pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: DelayCfg = params.clone().try_into()?;
    let millis = cfg.millis.unwrap_or(50);  // Значение по умолчанию

    Ok(Box::new(Delay {
        millis,
        side: bcx.side,  // Движок сообщает, на какой мы стороне
    }))
}
```

#### 5. Зарегистрируйте слой

Добавьте ваш слой в `layers/src/lib.rs`:

```rust
pub fn register(r: &mut Registry) {
    // ... существующие слои ...
    r.add("delay", wrappers::delay::build);  // Добавьте эту строку
}
```

Создайте файл модуля по пути `layers/src/wrappers/delay.rs` и добавьте `pub mod delay;` в `layers/src/wrappers/mod.rs`.

---

## Понимание `Ports`

`ports()` сообщает движку, что делает ваш слой. Выберите правильную константу:

| Константа | Принимает | Отдаёт | Случай использования |
|-----------|-----------|--------|----------------------|
| `Ports::SOURCE` | Ничего | `Stream` | Создаёт соединение (исходящий набор) |
| `Ports::LISTENER` | Ничего | `Streams` | Принимает соединения (входящий прослушиватель) |
| `Ports::WRAP` | `Stream` | `Stream` | Трансформирует один поток в другой |
| `Ports::ACCEPT` | `Stream` | `Streams` | Принимает запросы на потоке, создаёт несколько |
| `Ports::TERMINAL` | `Stream` | `Done` | Обработчик протокола — потребляет поток |

**Примеры:**
- `tcp` использует `SOURCE` (исходящий) / `LISTENER` (входящий)
- `tls` использует `WRAP` — принимает зашифрованный поток, отдаёт расшифрованный
- `http2` использует `ACCEPT` — принимает соединение, отдаёт виртуальные потоки
- `vless` использует `TERMINAL` — обрабатывает протокол, затем завершает

---

## Использование `requires()` и `provides()`

Слои общаются через общий объект `Cx` (контекст):

```rust
// Если вашему слою нужен ConnectRequest (например, чтобы знать, куда подключаться):
fn requires(&self) -> Vec<Key> {
    match self.side {
        Side::Outbound => vec![key::<ConnectRequest>()],
        _ => Vec::new(),
    }
}

// Доступ к нему в call():
let req = cx.get::<ConnectRequest>()
    .ok_or_else(|| anyhow!("нет ConnectRequest в Cx"))?;
```

Доступные ключи контекста:
- `ConnectRequest` — целевой адрес/порт (устанавливается входящими слоями протокола)
- `Dialer` — используется терминальными слоями для создания исходящих соединений

---

## Поведение, специфичное для стороны

Сохраните `side` в вашей структуре и разветвляйте логику в `call()`:

```rust
fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
    Box::pin(async move {
        match self.side {
            Side::Inbound => self.accept(input, cx).await,
            Side::Outbound => self.dial(input, cx).await,
        }
    })
}
```

Проверяйте конфигурацию в `build()` в зависимости от стороны:

```rust
pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: MyCfg = params.clone().try_into()?;
    match bcx.side {
        Side::Inbound => {
            if cfg.cert.is_none() {
                bail!("my-layer inbound требует 'cert'");
            }
        }
        Side::Outbound => {
            if cfg.sni.is_none() {
                bail!("my-layer outbound требует 'sni'");
            }
        }
    }
    // ...
}
```

---

## Реализация `AsyncRead` / `AsyncWrite`

Обёртки трансформируют потоки, реализуя `AsyncRead` и/или `AsyncWrite`. Паттерн одинаков для всех обёрток (`jitter`, `fragment`, `pad`).

### Базовый паттерн

```rust
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct MyStream<S> {
    inner: S,
    // ... ваше состояние ...
}

impl<S> MyStream<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

// Пропускное чтение (большинство обёрток не изменяют чтение)
impl<S: AsyncRead + Unpin> AsyncRead for MyStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

// Трансформация записи
impl<S: AsyncWrite + Unpin> AsyncWrite for MyStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Трансформировать buf, затем передать внутреннему
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
```

### Ключевые правила

1. **Ограничение `Unpin`** — внутренний поток должен быть `Unpin`, чтобы вызывать `Pin::new(&mut self.inner)`
2. **Возвращайте `Poll::Pending`** — если внутренний возвращает `Pending`, передайте его дальше
3. **Отслеживайте частичные записи** — `poll_write` может записать меньше байт, чем запрошено; верните фактическое количество
4. **Используйте макрос `ready!`** — для последовательных асинхронных операций в рамках одного poll

### Пример: Задержка записи (из `jitter.rs`)

```rust
impl<S: AsyncWrite + Unpin> AsyncWrite for JitterStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Проверяем, нужно ли сначала подождать
        if !self.slept {
            if !self.armed {
                let d = self.current_delay();
                if d.is_zero() {
                    self.slept = true;
                } else {
                    self.sleep.as_mut().reset(Instant::now() + d);
                    self.armed = true;
                }
            }
            if self.armed {
                ready!(self.sleep.as_mut().poll(cx));  // Ждём таймер
                self.armed = false;
                self.slept = true;
            }
        }

        // Передаём внутреннему потоку
        let r = ready!(Pin::new(&mut self.inner).poll_write(cx, buf));
        self.slept = false;  // Сбрасываем для следующей записи

        if let Ok(n) = r {
            self.written += n as u64;
        }
        Poll::Ready(r)
    }
    // ... flush и shutdown пропускаются
}
```

### Пример: Разделение записи (из `fragment.rs`)

```rust
impl<S: AsyncWrite + Unpin> AsyncWrite for FragmentStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut offset = 0;

        while offset < buf.len() {
            let current_buf = &buf[offset..];
            let take = self.calculate_take(current_buf.len());

            match Pin::new(&mut self.inner).poll_write(cx, &current_buf[..take]) {
                Poll::Ready(Ok(n)) => {
                    if n == 0 {
                        return Poll::Ready(Ok(offset));
                    }
                    self.written += n as u64;
                    offset += n;
                    if n < take {
                        break;  // Частичная запись, останавливаемся
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {
                    // Если что-то записали, сообщаем; иначе передаём Pending
                    return if offset > 0 {
                        Poll::Ready(Ok(offset))
                    } else {
                        Poll::Pending
                    };
                }
            }
        }

        Poll::Ready(Ok(offset))
    }
    // ...
}
```

### Пример: Фрейминг с внутренним буфером (из `pad.rs`)

Когда вашей обёртке нужно буферизировать данные (например, добавлять заголовки, заполнение), используйте внутренний буфер записи:

```rust
pub struct PadStream<S> {
    inner: S,
    wbuf: Vec<u8>,      // Ожидающие байты для записи
    woff: usize,        // Смещение в wbuf
    // ... другое состояние ...
}

impl<S: AsyncWrite + Unpin> PadStream<S> {
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.woff < self.wbuf.len() {
            let n = ready!(
                Pin::new(&mut self.inner).poll_write(cx, &self.wbuf[self.woff..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.woff += n;
        }
        self.wbuf.clear();
        self.woff = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PadStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        // Сначала выгружаем все ожидающие байты
        ready!(this.poll_drain(cx))?;

        // Строим фрейм в буфере
        let n = this.make_frame(buf);

        // Пытаемся отправить немедленно
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }

        Poll::Ready(Ok(n))  // Сообщаем длину исходного buf как потреблённую
    }
}
```

### Пример: Состоятельное чтение (из `pad.rs`)

Для протоколов с фреймингом отслеживайте состояние чтения:

```rust
enum ReadState {
    Hdr { got: usize },           // Чтение заголовка
    Payload { left: usize, pad: usize },  // Чтение полезной нагрузки
    Pad { left: usize },          // Пропуск заполнения
}

impl<S: AsyncRead + Unpin> AsyncRead for PadStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match this.rstate {
                ReadState::Hdr { got } => {
                    // Читаем во временный буфер
                    let mut rb = ReadBuf::new(&mut this.hdr[got..]);
                    ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                    let m = rb.filled().len();

                    if m == 0 {
                        return if got == 0 {
                            Poll::Ready(Ok(()))  // Чистый EOF
                        } else {
                            Poll::Ready(Err(eof("header")))
                        };
                    }
                    // ... обновляем состояние ...
                }
                // ... обрабатываем другие состояния ...
            }
        }
    }
}
```

---

## Добавление таймаута

```rust
fn default_timeout(&self) -> Option<Duration> {
    Some(Duration::from_secs(10))
}
```

---

## Логирование

Используйте тегированное логирование для согласованного вывода:

```rust
use cray_core::log_tag;

log_tag!(debug, "DELAY", "Обработка соединения");
log_tag!(trace, "DELAY", "Подробная информация: {:?}", data);
log_tag!(warn, "DELAY", "Что-то странное: {}", e);
```

Доступные теги: `CHAIN`, `INB`, `HTTP`, `TLS`, `VLESS`, `JITTER`, `FRAG`. Используйте имя вашего слоя в верхнем регистре.

---

## Рекомендации по PR

- Держите PR сфокусированными: одна фича или исправление на PR
- Добавляйте или обновляйте тесты для изменений, где это имеет смысл
- Обновляйте README (и `README.ru.md`), если ваше изменение влияет на документацию
- Запускайте `cargo fmt --all` и `cargo clippy --workspace` перед пушем

## Лицензия

Внося вклад, вы соглашаетесь с тем, что ваш вклад будет лицензирован под лицензией [GPL-3.0-or-later](./LICENSE).

---

Список участников смотрите в [CONTRIBUTORS.md](./CONTRIBUTORS.md).
