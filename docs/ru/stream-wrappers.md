# Обёртки потоков: реализация `AsyncRead` / `AsyncWrite`

Обёртки трансформируют потоки, реализуя `AsyncRead` и/или `AsyncWrite` на
структуре, которая держит внутренний поток. Все они живут в
[`layers/src/wrappers/`](../../layers/src/wrappers/): `jitter`, `fragment`,
`pad` (шейпинг записи), `auth` (фреймирование с проверкой). Паттерн один и
тот же во всех обёртках, поэтому страница идёт по нему на настоящем коде.

[Read in English ->](../eng/stream-wrappers.md)

Само тело слоя остаётся маленьким — слой принимает поток, заворачивает его и
передаёт дальше:

```rust
// layers/src/wrappers/jitter.rs
fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
    Box::pin(async move {
        let stream: BoxedStream = input.stream()?;
        Ok(Output::Stream(Box::pin(JitterStream::new(
            stream,
            self.stages.clone(),
        ))))
    })
}
```

## Базовый паттерн

```rust
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct MyStream<S> {
    inner: S,
    // ... ваше состояние ...
}

// Пропускное чтение (jitter, fragment и pad не меняют чтение; auth —
// исключение: он снимает своё фреймирование, у него машина состояний)
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

## Ключевые правила

1. **Ограничение `Unpin`** — внутренний поток должен быть `Unpin`, иначе
   не вызвать `Pin::new(&mut self.inner)`.
2. **Передавайте `Poll::Pending` дальше** — если внутренний вернул
   `Pending`, отдайте `Pending`; но если вы уже записали байты в этом
   вызове, отчитайтесь ими вместо повторного `Pending` (см. `fragment`
   ниже).
3. **Следите за частичными записями** — `poll_write` может записать меньше
   байт, чем просили; верните фактическое число и продолжайте с него в
   следующий вызов.
4. **Используйте макрос `ready!`** — для последовательных шагов внутри
   одного poll: если шаг ещё не готов, он сразу вернёт `Poll::Pending`.

## Пример: задержка записи (из `jitter.rs`)

`JitterStream` выдерживает случайную задержку перед передачей каждой записи.
Единственное поле `Box<Sleep>` заводится заново и переиспользуется между
poll-ами:

```rust
impl<S: AsyncWrite + Unpin> AsyncWrite for JitterStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
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
                ready!(self.sleep.as_mut().poll(cx));  // ждём таймер
                self.armed = false;
                self.slept = true;
            }
        }

        let r = ready!(Pin::new(&mut self.inner).poll_write(cx, buf));
        self.slept = false;  // сброс для следующей записи

        if let Ok(n) = r {
            self.written += n as u64;
        }
        Poll::Ready(r)
    }
    // ... flush и shutdown пропускаются насквозь
}
```

## Пример: нарезка записи (из `fragment.rs`)

`FragmentStream` режет одну запись на куски. Он перебирает остаток буфера и
обязан отчитаться частичной записью, если внутренний поток уходит в
`Pending` посреди цикла:

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

            while let Some(up) = self.stages[self.idx].bytes {
                if self.written >= up && self.idx + 1 < self.stages.len() {
                    self.idx += 1;
                } else {
                    break;
                }
            }

            let st = &self.stages[self.idx];
            let take = match st.bytes {
                Some(up) if self.written >= up => current_buf.len(),
                Some(up) => st
                    .size
                    .sample(current_buf.len())
                    .min((up - self.written) as usize)
                    .max(1),
                None => st.size.sample(current_buf.len()),
            }
            .min(current_buf.len());

            match Pin::new(&mut self.inner).poll_write(cx, &current_buf[..take]) {
                Poll::Ready(Ok(n)) => {
                    if n == 0 {
                        return Poll::Ready(Ok(offset));
                    }
                    self.written += n as u64;
                    offset += n;
                    if n < take {
                        break;  // частичная запись, останавливаемся
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {
                    if offset > 0 {
                        return Poll::Ready(Ok(offset));  // отчитываемся записанным
                    } else {
                        return Poll::Pending;
                    }
                }
            }
        }

        Poll::Ready(Ok(offset))
    }
    // ...
}
```

## Пример: фрейминг с внутренним буфером (из `pad.rs`)

`pad` оборачивает каждую запись в кадр `[payload_len:2 | pad_len:2 |
payload | pad]`. Обёртке, которой сначала нужен кадр в памяти, нужен
внутренний буфер записи (`wbuf`) и смещение в нём (`woff`); перед каждой
новой записью буфер выгружается во внутренний поток:

```rust
pub struct PadStream<S> {
    inner: S,
    wbuf: Vec<u8>,      // ожидающие записи байты
    woff: usize,        // смещение в wbuf
    // ... прочее состояние ...
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
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // Сначала выгружаем все ожидающие байты
        ready!(this.poll_drain(cx))?;

        // Строим кадр в буфере
        let n = this.make_frame(buf);

        // Пробуем отправить немедленно
        if let Poll::Ready(Err(e)) = this.poll_drain(cx) {
            return Poll::Ready(Err(e));
        }

        Poll::Ready(Ok(n))  // отчитываемся длиной исходного buf
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
```

Заметьте: `poll_write` отчитывается размером `n` — длиной **исходного**,
нефреймированного буфера, — потому что байты кадра живут в `wbuf`, а не в
`buf`.

## Пример: состоятельное чтение (из `pad.rs`)

Читатель фреймированного протокола в каждом `poll_read` ходит по машине
состояний: читает во временные буферы и отдаёт вызывающему только готовую
полезную нагрузку:

```rust
#[derive(Clone, Copy)]
enum ReadState {
    Hdr { got: usize },                     // читаем заголовок
    Payload { left: usize, pad: usize },    // читаем полезную нагрузку
    Pad { left: usize },                    // пропускаем паддинг
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
                    // читаем во временный буфер заголовка
                    let m = {
                        let mut rb = ReadBuf::new(&mut this.hdr[got..]);
                        ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                        rb.filled().len()
                    };

                    if m == 0 {
                        return if got == 0 {
                            Poll::Ready(Ok(()))  // чистый EOF между кадрами
                        } else {
                            Poll::Ready(Err(eof("header")))
                        };
                    }
                    // ... сдвигаем состояние, разбираем длины, переходим в Payload ...
                }
                // ... Payload копирует в ReadBuf вызывающего и возвращается;
                // Pad читает и выбрасывает паддинг ...
            }
        }
    }
}
```

`ReadBuf::new(&mut this.hdr[got..])` держит внешний буфер нетронутым, пока
готовый кусок полезной нагрузки не собран — паддинг вызывающий никогда не
видит.

## Тестирование

Набор `layers/tests/pad.rs` гоняет ровно те тяжёлые случаи, что описаны
выше: roundtrip через пару duplex, проверки формата на проводе по сырой
стороне, крошечный 100-байтный буфер duplex, вынуждающий частичность каждой
записи кадра (`survives_a_tiny_pipe`), EOF посреди кадра
(`eof_inside_a_frame_is_an_error`) и пустые кадры на проводе
(`empty_decoy_frames_are_skipped`). Паттерн тестов описан в
[writing-layers.md](writing-layers.md#как-протестировать-слой).
