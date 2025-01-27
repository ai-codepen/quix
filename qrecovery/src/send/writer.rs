use super::sender::{ArcSender, Sender};
use std::{
    io,
    ops::DerefMut,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

/// TODO: Drop视为自动cancel
#[derive(Debug)]
pub struct Writer(pub(super) ArcSender);

impl AsyncWrite for Writer {
    /// 往sndbuf里面写数据，直到写满MAX_STREAM_DATA，等通告窗口更新再写
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut sender = self.0.lock().unwrap();
        let inner = sender.deref_mut();
        match inner.take() {
            Sender::Ready(mut s) => {
                let result = s.poll_write(cx, buf);
                inner.replace(Sender::Ready(s));
                result
            }
            Sender::Sending(mut s) => {
                let result = s.poll_write(cx, buf);
                inner.replace(Sender::Sending(s));
                result
            }
            Sender::DataSent(s) => {
                inner.replace(Sender::DataSent(s));
                Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
            }
            Sender::DataRecvd => {
                inner.replace(Sender::DataRecvd);
                Poll::Ready(Err(io::ErrorKind::Unsupported.into()))
            }
            Sender::ResetSent(final_size) => {
                inner.replace(Sender::ResetSent(final_size));
                Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
            Sender::ResetRecvd => {
                inner.replace(Sender::ResetRecvd);
                Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut sender = self.0.lock().unwrap();
        let inner = sender.deref_mut();
        match inner.take() {
            Sender::Ready(mut s) => {
                let result = s.poll_flush(cx);
                inner.replace(Sender::Ready(s));
                result
            }
            Sender::Sending(mut s) => {
                let result = s.poll_flush(cx);
                inner.replace(Sender::Sending(s));
                result
            }
            Sender::DataSent(mut s) => {
                let result = s.poll_flush(cx);
                match &result {
                    Poll::Pending => inner.replace(Sender::DataSent(s)),
                    Poll::Ready(_) => inner.replace(Sender::DataRecvd),
                }
                result
            }
            Sender::DataRecvd => {
                inner.replace(Sender::DataRecvd);
                Poll::Ready(Ok(()))
            }
            Sender::ResetSent(final_size) => {
                inner.replace(Sender::ResetSent(final_size));
                Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
            Sender::ResetRecvd => {
                inner.replace(Sender::ResetRecvd);
                Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut sender = self.0.lock().unwrap();
        let inner = sender.deref_mut();
        match inner.take() {
            Sender::Ready(mut s) => {
                let result = s.poll_shutdown(cx);
                // 鉴于Ready是尚未分配StreamId的，所以还不具备直接变成DataSent资格
                // THINK: 如果将来实现的Sender，确实不需要StreamId选项，那可以直接
                // 转化成DataSent
                inner.replace(Sender::Ready(s));
                result
            }
            Sender::Sending(mut s) => {
                let result = s.poll_shutdown(cx);
                match &result {
                    Poll::Pending => inner.replace(Sender::DataSent(s.end())),
                    Poll::Ready(_) => inner.replace(Sender::DataRecvd),
                }
                // 有可能是Poll::Pending，也有可能是已经发送完数据的Poll::Ready
                result
            }
            Sender::DataSent(mut s) => {
                let result = s.poll_shutdown(cx);
                // 有一种复杂的情况，就是在DataSent途中，对方发来了STOP_SENDING，我方需立即
                // reset停止发送，此时状态也轮转到ResetSent中，相当于被动reset，再次唤醒该
                // poll任务，则会进到ResetSent或者ResetRecvd中poll，得到的将是BrokenPipe错误
                match &result {
                    Poll::Pending => inner.replace(Sender::DataSent(s)),
                    Poll::Ready(_) => inner.replace(Sender::DataRecvd),
                }
                result
            }
            Sender::DataRecvd => {
                inner.replace(Sender::DataRecvd);
                Poll::Ready(Ok(()))
            }
            Sender::ResetSent(final_size) => {
                inner.replace(Sender::ResetSent(final_size));
                Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
            Sender::ResetRecvd => {
                inner.replace(Sender::ResetRecvd);
                Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let mut sender = self.0.lock().unwrap();
        let inner = sender.deref_mut();
        match inner.take() {
            Sender::Ready(mut s) => {
                s.cancel();
                inner.replace(Sender::Ready(s));
            }
            Sender::Sending(mut s) => {
                s.cancel();
                inner.replace(Sender::Sending(s));
            }
            Sender::DataSent(mut s) => {
                s.cancel();
                inner.replace(Sender::DataSent(s));
            }
            other => {
                inner.replace(other);
            }
        };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
