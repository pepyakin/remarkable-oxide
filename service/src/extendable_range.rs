use futures::prelude::*;

/// This stream is similar to a range iterator: for a range `[lhs..rhs]` it emits all numbers in
/// sequence starting from `lhs` to `rhs`.
///
/// The feature of this stream is that `rhs` can be moved forward. In fact, the `rhs` specified by
/// the `rhs_stream`. Each time `rhs_stream` produces a number, it will extend `rhs` further, making
/// this stream to produce numbers to the new rhs. Thus extendable in the name.
pub fn extendable_range(
    start_num: u64,
    rhs_stream: impl Stream<Item = u64> + Unpin,
) -> impl Stream<Item = u64> {
    struct State<F> {
        lhs: u64,
        rhs: u64,
        rhs_stream: F,
    }
    stream::unfold(
        State {
            lhs: start_num,
            rhs: start_num,
            rhs_stream,
        },
        |mut state| async move {
            let lhs = state.lhs;
            if state.lhs >= state.rhs {
                loop {
                    let next_rhs = state.rhs_stream.next().await?;
                    if next_rhs > state.rhs {
                        state.rhs = next_rhs;
                        break;
                    }
                }
            }
            state.lhs += 1;
            Some((lhs, state))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::extendable_range;
    use async_std::task;
    use futures::channel::mpsc;
    use futures::prelude::*;
    use std::time::Duration;

    #[async_std::test]
    async fn initial_send() {
        let (mut rhs_stream_tx, rhs_stream_rx) = mpsc::unbounded();
        rhs_stream_tx.send(10).await.unwrap();
        let range = extendable_range(0, rhs_stream_rx)
            .take(10)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(range, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[async_std::test]
    async fn with_task() {
        let (mut rhs_stream_tx, rhs_stream_rx) = mpsc::unbounded();
        task::spawn(async move {
            rhs_stream_tx.send(5).await.unwrap();
            rhs_stream_tx.send(10).await.unwrap();
        })
        .await;
        let range = task::spawn(
            extendable_range(0, rhs_stream_rx)
                .take(10)
                .collect::<Vec<_>>(),
        );
        assert_eq!(range.await, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }
}
