package wrpcnats

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"log/slog"

	"github.com/nats-io/nats.go"
	wrpc "github.com/wrpc/wrpc/go"
)

type headerKey struct{}

func HeaderFromContext(ctx context.Context) (nats.Header, bool) {
	v, ok := ctx.Value(headerKey{}).(nats.Header)
	return v, ok
}

func ContextWithHeader(ctx context.Context, header nats.Header) context.Context {
	return context.WithValue(ctx, headerKey{}, header)
}

func errorSubject(prefix string) string {
	return fmt.Sprintf("%s.error", prefix)
}

func paramSubject(prefix string) string {
	return fmt.Sprintf("%s.params", prefix)
}

func resultSubject(prefix string) string {
	return fmt.Sprintf("%s.results", prefix)
}

func invocationSubject(prefix string, instance string, name string) string {
	subject := fmt.Sprintf("wrpc.0.0.1.%s.%s", instance, name)
	if prefix != "" {
		return fmt.Sprintf("%s.%s", prefix, subject)
	}
	return subject
}

func subscribe(conn *nats.Conn, prefix string, f func(context.Context, []byte), path ...uint32) (*nats.Subscription, error) {
	subject := prefix
	for _, p := range path {
		subject = fmt.Sprintf("%s.%d", subject, p)
	}
	return conn.Subscribe(subject, func(m *nats.Msg) {
		ctx := context.Background()
		ctx = ContextWithHeader(ctx, m.Header)
		f(ctx, m.Data)
	})
}

func transmit(ctx context.Context, conn *nats.Conn, subject string, reply string, buf []byte) error {
	header, hasHeader := HeaderFromContext(ctx)
	m := nats.NewMsg(subject)
	m.Reply = reply
	if hasHeader {
		m.Header = header
	}

	maxPayload := conn.MaxPayload()
	mSize := int64(m.Size())
	if mSize > maxPayload {
		return fmt.Errorf("message size %d is larger than maximum allowed payload size %d", mSize, maxPayload)
	}
	maxPayload -= mSize
	maxPayload = min(maxPayload, int64(len(buf)))
	m.Data, buf = buf[:maxPayload], buf[maxPayload:]
	if err := conn.PublishMsg(m); err != nil {
		return fmt.Errorf("failed to send initial payload chunk: %w", err)
	}
	for len(buf) > 0 {
		m := nats.NewMsg(subject)
		m.Reply = reply
		if hasHeader {
			m.Header = header
		}
		maxPayload = min(maxPayload, int64(len(buf)))
		m.Data, buf = buf[:maxPayload], buf[maxPayload:]
		if err := conn.PublishMsg(m); err != nil {
			return fmt.Errorf("failed to send payload chunk: %w", err)
		}
	}
	return nil
}

type invocation struct {
	conn *nats.Conn
	rx   string
	tx   string
}

func (inv *invocation) SubscribeError(f func(context.Context, []byte)) (func() error, error) {
	sub, err := inv.conn.Subscribe(fmt.Sprintf("%s.error", inv.rx), func(m *nats.Msg) {
		ctx := context.Background()
		ctx = ContextWithHeader(ctx, m.Header)
		f(ctx, m.Data)
	})
	if err != nil {
		return nil, fmt.Errorf("failed to subscribe for error: %w", err)
	}
	return sub.Unsubscribe, nil
}

type OutgoingInvocation struct{ invocation }

func (c *Client) NewInvocation(instance string, name string) wrpc.OutgoingInvocation {
	return &OutgoingInvocation{
		invocation: invocation{
			conn: c.conn,
			rx:   nats.NewInbox(),
			tx:   invocationSubject(c.prefix, instance, name),
		},
	}
}

func (inv *OutgoingInvocation) Subscribe(f func(context.Context, []byte), path ...uint32) (func() error, error) {
	sub, err := subscribe(inv.conn, resultSubject(inv.rx), f, path...)
	if err != nil {
		return nil, fmt.Errorf("failed to subscribe for results: %w", err)
	}
	return sub.Unsubscribe, nil
}

func (inv *OutgoingInvocation) SubscribeError(f func(context.Context, []byte)) (func() error, error) {
	return inv.invocation.SubscribeError(f)
}

func (inv *OutgoingInvocation) Invoke(ctx context.Context, buf []byte, f func(context.Context, []byte)) (func() error, wrpc.Transmitter, error) {
	txCh := make(chan chan<- string, 1)
	sub, err := inv.conn.Subscribe(inv.rx, func(m *nats.Msg) {
		ctx := context.Background()
		ctx = ContextWithHeader(ctx, m.Header)
		reply, ok := <-txCh
		if !ok {
			slog.DebugContext(ctx, "handshake reply channel closed")
			return
		}
		reply <- m.Reply
		close(reply)
		f(ctx, m.Data)
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to subscribe for handshake: %w", err)
	}
	if err := transmit(ctx, inv.conn, inv.tx, "", buf); err != nil {
		return sub.Unsubscribe, nil, fmt.Errorf("failed to transmit handshake: %w", err)
	}
	reply := make(chan string, 1)
	select {
	case txCh <- reply:
		subject := <-reply
		return sub.Unsubscribe, &Transmitter{
			conn:    inv.conn,
			subject: subject,
		}, nil
	case <-ctx.Done():
		return sub.Unsubscribe, nil, ctx.Err()
	}
}

type IncomingInvocation struct{ invocation }

func (inv *IncomingInvocation) Subscribe(f func(context.Context, []byte), path ...uint32) (func() error, error) {
	sub, err := subscribe(inv.conn, paramSubject(inv.rx), f, path...)
	if err != nil {
		return nil, fmt.Errorf("failed to subscribe for parameters: %w", err)
	}
	return sub.Unsubscribe, nil
}

func (inv *IncomingInvocation) SubscribeError(f func(context.Context, []byte)) (func() error, error) {
	return inv.invocation.SubscribeError(f)
}

func (inv *IncomingInvocation) Accept(ctx context.Context, buf []byte) error {
	if err := transmit(ctx, inv.conn, inv.tx, inv.rx, buf); err != nil {
		return fmt.Errorf("failed to transmit accept: %w", err)
	}
	return nil
}

type Transmitter struct {
	conn    *nats.Conn
	subject string
}

func (tx *Transmitter) Transmit(ctx context.Context, buf []byte, path ...uint32) error {
	subject := tx.subject
	for _, p := range path {
		subject = fmt.Sprintf("%s.%d", subject, p)
	}
	return transmit(ctx, tx.conn, subject, "", buf)
}

type Client struct {
	conn   *nats.Conn
	prefix string
}

func NewClient(conn *nats.Conn, prefix string) *Client {
	return &Client{conn, prefix}
}

func (c *Client) Serve(instance string, name string, f func(context.Context, []byte, wrpc.Transmitter, wrpc.IncomingInvocation) error) (stop func() error, err error) {
	sub, err := c.conn.Subscribe(invocationSubject(c.prefix, instance, name), func(m *nats.Msg) {
		slog.Debug("received invocation", "instance", instance, "name", name)
		if m.Reply == "" {
			slog.Warn("peer did not specify a reply subject")
			return
		}
		ctx := context.Background()
		ctx = ContextWithHeader(ctx, m.Header)
		slog.Debug("calling handler")
		if err := f(ctx, m.Data, &Transmitter{
			conn:    c.conn,
			subject: resultSubject(m.Reply),
		}, &IncomingInvocation{
			invocation: invocation{
				conn: c.conn,
				rx:   nats.NewInbox(),
				tx:   m.Reply,
			},
		}); err != nil {
			var buf bytes.Buffer
			slog.Warn("failed to handle `handle`", "err", err)
			if err = wrpc.WriteString(fmt.Sprintf("%s", err), &buf); err != nil {
				slog.Warn("failed to encode `handle` handling error", "err", err)
				// Encoding the error failed, let's try encoding the encoding error
				if err = wrpc.WriteString(fmt.Sprintf("failed to encode error: %s", err), &buf); err != nil {
					slog.Warn("failed to encode `handle` handling error encoding error", "err", err)
					// Well, we're out of luck at this point, let's just send an empty string
					buf.Reset()
				}
			}
			slog.Debug("transmitting error")
			if err = transmit(context.Background(), c.conn, fmt.Sprintf("%s.error", m.Reply), "", buf.Bytes()); err != nil {
				slog.Warn("failed to send error to client", "err", err)
			}
			return
		}
		slog.Debug("successfully finished serving invocation")
	})
	if err != nil {
		return nil, fmt.Errorf("failed to serve `%s` for instance `%s`: %w", name, instance, err)
	}
	return sub.Unsubscribe, nil
}

type paramWriter struct {
	ctx context.Context
	nc  *nats.Conn
	rx  string
	tx  string
}

func (w *paramWriter) Write(p []byte) (int, error) {
	if w.nc == nil {
		return 0, errors.New("writer is invalid")
	}
	defer func() {
		*w = paramWriter{}
	}()

	maxPayload := w.nc.MaxPayload()
	if int64(len(p)) <= maxPayload {
		if err := w.nc.Publish(w.tx, p); err != nil {
			return 0, fmt.Errorf("failed to send payload: %w", err)
		}
	}

	sub, err := w.nc.SubscribeSync(w.rx)
	if err != nil {
		return 0, fmt.Errorf("failed to subscribe payload: %w", err)
	}

	n := len(p)
	maxPayload = min(maxPayload, int64(n))
	var buf []byte
	p, buf = p[:maxPayload], p[maxPayload:]
	if err := w.nc.Publish(w.tx, p); err != nil {
		return 0, fmt.Errorf("failed to send initial payload chunk: %w", err)
	}
	msg, err := sub.NextMsgWithContext(w.ctx)
	if err != nil {
		return 0, fmt.Errorf("failed to receive handshake: %w", err)
	}
	w.tx = fmt.Sprintf("%s.params", msg.Reply)
	for len(buf) > 0 {
		maxPayload = min(maxPayload, int64(len(buf)))
		p, buf = buf[:maxPayload], buf[maxPayload:]
		if err := w.nc.Publish(w.tx, p); err != nil {
			return 0, fmt.Errorf("failed to send payload chunk: %w", err)
		}
	}
	return n, nil
}

func (w *paramWriter) WriteByte(b byte) error {
	if err := w.nc.Publish(w.tx, []byte{b}); err != nil {
		return fmt.Errorf("failed to send byte: %w", err)
	}
	return nil
}

func (w *paramWriter) Index(path ...uint32) (wrpc.IndexWriter, error) {
	return nil, errors.New("indexing not supported yet")
}

func (c *Client) Invoke(instance string, name string, w wrpc.WriterToIndex, subs ...[]*uint32) (wrpc.IndexReader, <-chan error, error) {
	rx := nats.NewInbox()

	resultRx := fmt.Sprintf("%s.results", rx)
	resultSub, err := c.conn.SubscribeSync(resultRx)
	if err != nil {
		return nil, nil, fmt.Errorf("failed to subscribe on result subject `%s`: %w", resultRx, err)
	}
	defer func() {
		if err := resultSub.Unsubscribe(); err != nil {
			slog.Error("failed to unsubscribe from result subject", "err", err)
		}
	}()

	errRx := fmt.Sprintf("%s.error", rx)
	errPayloadCh := make(chan []byte, 1)
	errSub, err := c.conn.Subscribe(errRx, func(m *nats.Msg) {
		errPayloadCh <- m.Data
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to subscribe on error subject `%s`: %w", errRx, err)
	}
	defer func() {
		if err := errSub.Drain(); err != nil {
			slog.Error("failed to drain error subject", "err", err)
		}
	}()
	// TODO: subscribe on the paths
	//for sub := range subs {
	//}
	panic("todo")
}

type streamReader struct {
	ctx context.Context
	sub *nats.Subscription
	buf []byte
}

func (r *streamReader) Read(p []byte) (int, error) {
	if len(r.buf) > 0 {
		n := copy(p, r.buf)
		slog.Debug("copied bytes from buffer", "requested", len(p), "buffered", len(r.buf), "copied", n)
		r.buf = r.buf[n:]
		return n, nil
	}
	slog.Debug("receiving next byte chunk")
	msg, err := r.sub.NextMsgWithContext(r.ctx)
	if err != nil {
		return 0, err
	}
	n := copy(p, msg.Data)
	r.buf = msg.Data[n:]
	return n, nil
}

func (r *streamReader) ReadByte() (byte, error) {
	if len(r.buf) > 0 {
		b := r.buf[0]
		slog.Debug("copied byte from buffer", "buffered", len(r.buf))
		r.buf = r.buf[1:]
		return b, nil
	}
	for {
		slog.Debug("receiving next byte chunk")
		msg, err := r.sub.NextMsgWithContext(r.ctx)
		if err != nil {
			return 0, err
		}
		if len(msg.Data) == 0 {
			continue
		}
		r.buf = msg.Data[1:]
		return msg.Data[0], nil
	}
}

func (r *streamReader) Index(path ...uint32) (wrpc.IndexReader, error) {
	return nil, errors.New("indexing not supported yet")
}

type resultWriter struct {
	nc *nats.Conn
	tx string
}

func (w *resultWriter) Write(p []byte) (int, error) {
	n := len(p)
	maxPayload := w.nc.MaxPayload()
	maxPayload = min(maxPayload, int64(n))
	var buf []byte
	p, buf = p[:maxPayload], p[maxPayload:]
	if err := w.nc.Publish(w.tx, p); err != nil {
		return 0, fmt.Errorf("failed to send initial payload chunk: %w", err)
	}
	for len(buf) > 0 {
		maxPayload = min(maxPayload, int64(len(buf)))
		p, buf = buf[:maxPayload], buf[maxPayload:]
		if err := w.nc.Publish(w.tx, p); err != nil {
			return 0, fmt.Errorf("failed to send payload chunk: %w", err)
		}
	}
	return n, nil
}

func (w *resultWriter) WriteByte(b byte) error {
	if err := w.nc.Publish(w.tx, []byte{b}); err != nil {
		return fmt.Errorf("failed to send byte: %w", err)
	}
	return nil
}

func (w *resultWriter) Index(path ...uint32) (wrpc.IndexWriter, error) {
	return nil, errors.New("indexing not supported yet")
}

func (c *Client) ServeIndex(instance string, name string, f func(context.Context, wrpc.IndexReader, <-chan error) (wrpc.WriterToIndex, error), subs ...[]*uint32) (stop func() error, err error) {
	sub, err := c.conn.Subscribe(invocationSubject(c.prefix, instance, name), func(m *nats.Msg) {
		slog.Debug("received invocation", "instance", instance, "name", name)
		if m.Reply == "" {
			slog.Warn("peer did not specify a reply subject")
			return
		}

		rx := nats.NewInbox()

		paramRx := fmt.Sprintf("%s.params", rx)
		paramSub, err := c.conn.SubscribeSync(paramRx)
		if err != nil {
			slog.Warn("failed to subscribe on parameter subject", "subject", paramRx, "err", err)
			return
		}
		defer func() {
			if err := paramSub.Unsubscribe(); err != nil {
				slog.Error("failed to unsubscribe from parameter subject", "err", err)
			}
		}()

		errRx := fmt.Sprintf("%s.error", rx)
		errPayloadCh := make(chan []byte, 1)
		errSub, err := c.conn.Subscribe(errRx, func(m *nats.Msg) {
			errPayloadCh <- m.Data
		})
		if err != nil {
			slog.Warn("failed to subscribe on error subject", "subject", errRx, "err", err)
			return
		}
		defer func() {
			if err := errSub.Drain(); err != nil {
				slog.Error("failed to drain error subject", "err", err)
			}
		}()

		// TODO: subscribe on the paths
		//for sub := range subs {
		//}

		accept := nats.NewMsg(m.Reply)
		accept.Reply = rx
		if err := c.conn.PublishMsg(accept); err != nil {
			slog.Error("failed to send handshake", "err", err)
			return
		}

		ctx := context.Background()
		ctx, cancel := context.WithCancel(ctx)
		defer cancel()
		ctx = ContextWithHeader(ctx, m.Header)

		errCh := make(chan error, 1)
		go func() {
			defer close(errCh)
			s, err := wrpc.ReadString(wrpc.NewChanReader(ctx, errPayloadCh, nil))
			if err == context.Canceled {
				return
			}
			if err != nil {
				errCh <- fmt.Errorf("failed to read error string: %w", err)
			}
			errCh <- errors.New(s)
		}()

		slog.Debug("calling handler")
		w, err := f(ctx, &streamReader{
			ctx: ctx,
			buf: m.Data,
			sub: paramSub,
		}, errCh)
		if err != nil {
			var buf bytes.Buffer
			slog.Warn("failed to handle `handle`", "err", err)
			if err = wrpc.WriteString(fmt.Sprintf("%s", err), &buf); err != nil {
				slog.Warn("failed to encode `handle` handling error", "err", err)
				// Encoding the error failed, let's try encoding the encoding error
				if err = wrpc.WriteString(fmt.Sprintf("failed to encode error: %s", err), &buf); err != nil {
					slog.Warn("failed to encode `handle` handling error encoding error", "err", err)
					// Well, we're out of luck at this point, let's just send an empty string
					buf.Reset()
				}
			}
			slog.Debug("transmitting error")
			if err = transmit(context.Background(), c.conn, fmt.Sprintf("%s.error", m.Reply), "", buf.Bytes()); err != nil {
				slog.Warn("failed to send error to client", "err", err)
			}
			return
		}
		if err := w.WriteToIndex(&resultWriter{
			nc: c.conn,
			tx: fmt.Sprintf("%s.results", m.Reply),
		}); err != nil {
			slog.Warn("failed to write response to client", "err", err)
		}
		slog.Debug("successfully finished serving invocation")
	})
	if err != nil {
		return nil, fmt.Errorf("failed to serve `%s` for instance `%s`: %w", name, instance, err)
	}
	return sub.Unsubscribe, nil
}
