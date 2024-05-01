package wrpcnats

import (
	"bytes"
	"context"
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
			slog.Debug("transmitting error", "err", err)
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
