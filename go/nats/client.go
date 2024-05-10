package wrpcnats

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync"

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

func indexPath(prefix string, path ...uint32) string {
	s := prefix
	for _, p := range path {
		if s != "" {
			s = fmt.Sprintf("%s.%d", s, p)
		} else {
			s = fmt.Sprintf("%d", p)
		}
	}
	return s
}

func subscribePath(prefix string, path wrpc.SubscribePath) string {
	s := prefix
	for _, p := range path {
		if s != "" {
			s = fmt.Sprintf("%s.", s)
		}
		if p == nil {
			s = fmt.Sprintf("%s*", s)
		} else {
			s = fmt.Sprintf("%s%d", s, *p)
		}
	}
	return s
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

func transmitError(nc *nats.Conn, subject string, err error) error {
	var buf bytes.Buffer
	if err := wrpc.WriteString(fmt.Sprintf("%s", err), &buf); err != nil {
		slog.Warn("failed to encode handling error", "err", err)
		if err := wrpc.WriteString(fmt.Sprintf("failed to encode error: %s", err), &buf); err != nil {
			slog.Warn("failed to encode handling error encoding error", "err", err)
			buf.Reset()
		}
	}
	maxPayload := nc.MaxPayload()
	maxPayload = min(maxPayload, int64(buf.Len()))
	b := buf.Bytes()
	var tail []byte
	b, tail = b[:maxPayload], b[maxPayload:]
	slog.Debug("transmitting initial error chunk")
	if err := nc.Publish(subject, b); err != nil {
		return fmt.Errorf("failed to send initial error chunk: %w", err)
	}
	for len(tail) > 0 {
		maxPayload = min(maxPayload, int64(len(tail)))
		b, tail = b[:maxPayload], b[maxPayload:]
		slog.Debug("transmitting error chunk")
		if err := nc.Publish(subject, b); err != nil {
			return fmt.Errorf("failed to send error chunk: %w", err)
		}
	}
	return nil
}

type Client struct {
	conn   *nats.Conn
	prefix string
}

func NewClient(conn *nats.Conn, prefix string) *Client {
	return &Client{conn, prefix}
}

type paramWriter struct {
	ctx  context.Context
	nc   *nats.Conn
	rx   string
	tx   string
	init bool
}

func (w *paramWriter) publish(p []byte) (int, error) {
	maxPayload := w.nc.MaxPayload()
	pn := len(p)
	if !w.init {
		header, hasHeader := HeaderFromContext(w.ctx)
		m := nats.NewMsg(w.tx)
		m.Reply = w.rx
		if hasHeader {
			m.Header = header
		}
		mSize := int64(m.Size())
		if mSize > maxPayload {
			return 0, fmt.Errorf("message size %d is larger than maximum allowed payload size %d", mSize, maxPayload)
		}
		maxPayload -= mSize
		maxPayload = min(maxPayload, int64(len(p)))
		m.Data, p = p[:maxPayload], p[maxPayload:]

		sub, err := w.nc.SubscribeSync(w.rx)
		if err != nil {
			return 0, fmt.Errorf("failed to subscribe on Rx subject: %w", err)
		}
		slog.DebugContext(w.ctx, "publishing handshake", "rx", m.Reply)
		if err := w.nc.PublishMsg(m); err != nil {
			return 0, fmt.Errorf("failed to send initial payload chunk: %w", err)
		}
		n := len(m.Data)

		m, err = sub.NextMsgWithContext(w.ctx)
		if err != nil {
			return n, fmt.Errorf("failed to receive handshake: %w", err)
		}
		if m.Reply == "" {
			return n, errors.New("peer did not specify a reply subject")
		}
		w.tx = paramSubject(m.Reply)
		w.init = true
	}
	buf := p
	for len(buf) > 0 {
		maxPayload = min(maxPayload, int64(len(buf)))
		p, buf = buf[:maxPayload], buf[maxPayload:]
		if err := w.nc.Publish(w.tx, p); err != nil {
			return 0, fmt.Errorf("failed to send payload chunk: %w", err)
		}
	}
	return pn, nil
}

func (w *paramWriter) Write(p []byte) (int, error) {
	return w.publish(p)
}

func (w *paramWriter) WriteByte(b byte) error {
	_, err := w.publish([]byte{b})
	if err != nil {
		return err
	}
	return nil
}

func (w *paramWriter) Index(path ...uint32) (wrpc.IndexWriter, error) {
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
	return &resultWriter{nc: w.nc, tx: indexPath(w.tx, path...)}, nil
}

type subReader struct {
	sub    *nats.Subscription
	ctx    context.Context
	buf    []byte
	cancel func()
}

func (r *subReader) Read(p []byte) (int, error) {
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

func (r *subReader) ReadByte() (byte, error) {
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

type streamReader struct {
	*subReader
	err    *nats.Subscription
	nest   map[string]*nats.Subscription
	nestMu sync.Mutex
}

func (r *streamReader) Close() (err error) {
	defer r.cancel()

	r.nestMu.Lock()
	defer r.nestMu.Unlock()
	defer func() {
		if sErr := r.err.Unsubscribe(); sErr != nil {
			if err == nil {
				err = fmt.Errorf("failed to unsubscribe from error subject: %w", err)
			} else {
				slog.Error("failed to unsubscribe from error subject", "err", err)
			}
		}
	}()
	for path, sub := range r.nest {
		path := path
		sub := sub
		defer func() {
			if sErr := sub.Unsubscribe(); sErr != nil {
				if err == nil {
					err = fmt.Errorf("failed to unsubscribe from nested path `%s`: %w", path, sErr)
				} else {
					slog.Error("failed to unsubscribe from nested path", "path", path, "err", sErr)
				}
			}
		}()
	}
	p, n, err := r.err.Pending()
	if err != nil {
		return fmt.Errorf("failed to check pending error bytes: %w", err)
	}
	if p == 0 {
		return nil
	}
	if n == 0 {
		return errors.New("received an empty error")
	}
	slog.DebugContext(r.ctx, "reading error string")
	s, err := wrpc.ReadString(&subReader{
		ctx: r.ctx,
		sub: r.err,
	})
	if err == context.Canceled {
		return err
	}
	if err != nil {
		return fmt.Errorf("failed to read error string: %w", err)
	}
	return errors.New(s)
}

type indexedStreamReader struct {
	*streamReader
	sub  *nats.Subscription
	path string
	buf  []byte
}

func (r *streamReader) Index(path ...uint32) (wrpc.IndexReader, error) {
	s := indexPath("", path...)
	r.nestMu.Lock()
	defer r.nestMu.Unlock()
	sub, ok := r.nest[s]
	if !ok {
		return nil, errors.New("unknown subscription")
	}
	delete(r.nest, s)
	return &indexedStreamReader{
		r, sub, s, nil,
	}, nil
}

func (r *indexedStreamReader) Read(p []byte) (int, error) {
	if len(r.buf) > 0 {
		n := copy(p, r.buf)
		slog.Debug("copied bytes from buffer", "requested", len(p), "buffered", len(r.buf), "copied", n)
		r.buf = r.buf[n:]
		return n, nil
	}
	slog.Debug("receiving next byte chunk", "path", r.path)
	msg, err := r.sub.NextMsgWithContext(r.ctx)
	if err != nil {
		return 0, err
	}
	n := copy(p, msg.Data)
	r.buf = msg.Data[n:]
	return n, nil
}

func (r *indexedStreamReader) ReadByte() (byte, error) {
	if len(r.buf) > 0 {
		b := r.buf[0]
		slog.Debug("copied byte from buffer", "buffered", len(r.buf))
		r.buf = r.buf[1:]
		return b, nil
	}
	for {
		slog.Debug("receiving next byte chunk", "path", r.path)
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

func (r *indexedStreamReader) Index(path ...uint32) (wrpc.IndexReader, error) {
	s := indexPath(r.path, path...)
	r.nestMu.Lock()
	defer r.nestMu.Unlock()
	sub, ok := r.nest[s]
	if !ok {
		return nil, errors.New("unknown subscription")
	}
	delete(r.nest, s)
	return &indexedStreamReader{
		r.streamReader, sub, s, nil,
	}, nil
}

func (c *Client) Invoke(ctx context.Context, instance string, name string, f func(wrpc.IndexWriter, wrpc.IndexReadCloser) error, subs ...wrpc.SubscribePath) (err error) {
	ctx, cancel := context.WithCancel(ctx)

	rx := nats.NewInbox()

	resultRx := resultSubject(rx)
	slog.Debug("subscribing on result subject", "subject", resultRx)
	resultSub, err := c.conn.SubscribeSync(resultRx)
	if err != nil {
		cancel()
		return fmt.Errorf("failed to subscribe on result subject `%s`: %w", resultRx, err)
	}
	defer func() {
		if sErr := resultSub.Unsubscribe(); sErr != nil {
			if err == nil {
				err = fmt.Errorf("failed to unsubscribe from result subject: %w", sErr)
			} else {
				slog.Error("failed to unsubscribe from result subject", "err", sErr)
			}
		}
	}()

	errRx := errorSubject(rx)
	slog.Debug("subscribing on error subject", "subject", errRx)
	errSub, err := c.conn.SubscribeSync(errRx)
	if err != nil {
		cancel()
		return fmt.Errorf("failed to subscribe on error subject `%s`: %w", errRx, err)
	}

	nest := make(map[string]*nats.Subscription, len(subs))
	for _, path := range subs {
		s := subscribePath(resultRx, path)
		slog.Debug("subscribing on nested result subject", "subject", s)
		sub, err := c.conn.SubscribeSync(s)
		if err != nil {
			cancel()
			return fmt.Errorf("failed to subscribe on nested result subject `%s`: %w", s, err)
		}
		nest[subscribePath("", path)] = sub
	}

	slog.Debug("calling client handler")
	w := &paramWriter{
		ctx: ctx,
		nc:  c.conn,
		rx:  rx,
		tx:  invocationSubject(c.prefix, instance, name),
	}
	if err = f(w, &streamReader{
		subReader: &subReader{
			ctx:    ctx,
			sub:    resultSub,
			cancel: cancel,
		},
		err:  errSub,
		nest: nest,
	}); err != nil && w.init {
		if err := transmitError(c.conn, errorSubject(w.tx), err); err != nil {
			slog.Warn("failed to send error to server", "err", err)
		}
	}
	return err
}

func (c *Client) Serve(instance string, name string, f func(context.Context, wrpc.IndexWriter, wrpc.IndexReadCloser) error, subs ...wrpc.SubscribePath) (stop func() error, err error) {
	sub, err := c.conn.Subscribe(invocationSubject(c.prefix, instance, name), func(m *nats.Msg) {
		ctx := context.Background()
		ctx, cancel := context.WithCancel(ctx)
		ctx = ContextWithHeader(ctx, m.Header)

		slog.Debug("received invocation", "instance", instance, "name", name, "payload", m.Data, "reply", m.Reply)
		if m.Reply == "" {
			cancel()
			slog.Warn("peer did not specify a reply subject")
			return
		}

		rx := nats.NewInbox()

		paramRx := paramSubject(rx)
		slog.Debug("subscribing on parameter subject", "subject", paramRx)
		paramSub, err := c.conn.SubscribeSync(paramRx)
		if err != nil {
			cancel()
			slog.Warn("failed to subscribe on parameter subject", "subject", paramRx, "err", err)
			return
		}
		defer func() {
			if err := paramSub.Unsubscribe(); err != nil {
				slog.Error("failed to unsubscribe from parameter subject", "subject", paramRx, "err", err)
			}
		}()

		errRx := errorSubject(rx)
		slog.Debug("subscribing on error subject", "subject", errRx)
		errSub, err := c.conn.SubscribeSync(errRx)
		if err != nil {
			cancel()
			slog.Warn("failed to subscribe on error subject", "subject", errRx, "err", err)
			return
		}

		nest := make(map[string]*nats.Subscription, len(subs))
		for _, path := range subs {
			s := subscribePath(paramRx, path)
			slog.Debug("subscribing on nested parameter subject", "subject", s)
			sub, err := c.conn.SubscribeSync(s)
			if err != nil {
				cancel()
				slog.Warn("failed to subscribe on nested parameter subject", "subject", s, "err", err)
				return
			}
			nest[subscribePath("", path)] = sub
		}

		slog.DebugContext(ctx, "publishing handshake accept", "subject", m.Reply, "reply", rx)
		accept := nats.NewMsg(m.Reply)
		accept.Reply = rx
		if err := c.conn.PublishMsg(accept); err != nil {
			cancel()
			slog.Error("failed to send handshake", "err", err)
			return
		}

		slog.Debug("calling server handler")
		if err := f(ctx, &resultWriter{
			nc: c.conn,
			tx: resultSubject(m.Reply),
		}, &streamReader{
			subReader: &subReader{
				ctx:    ctx,
				sub:    paramSub,
				buf:    m.Data,
				cancel: cancel,
			},
			err:  errSub,
			nest: nest,
		}); err != nil {
			slog.Warn("failed to handle invocation", "err", err)
			if err := transmitError(c.conn, errorSubject(m.Reply), err); err != nil {
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
