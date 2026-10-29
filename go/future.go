package wrpc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
)

// ReadFutureStatus reads a single byte from `r` and returns:
// - `true` if future is "ready"
// - `false` if future is "pending"
func ReadFutureStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `future` status byte: %w", err)
	}
	switch status {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, fmt.Errorf("invalid `future` status byte %d", status)
	}
}

// ReadFuture reads a `future` from `r` using `f` to read its value.
// The returned channel receives the value once the future resolves and is closed afterwards.
// If the future is pending, the returned function reads it and must be called. If reading fails,
// the channel never receives a value and is never closed.
func ReadFuture[T any](r IndexReadCloser, f func(IndexReadCloser, ...uint32) (T, func(context.Context) error, error), path ...uint32) (<-chan T, func(context.Context) error, error) {
	slog.Debug("reading future status byte")
	ok, err := ReadFutureStatus(r)
	if err != nil {
		return nil, nil, err
	}
	ch := make(chan T, 1)
	if ok {
		slog.Debug("reading ready future")
		v, deferred, err := f(r, path...)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to read ready future: %w", err)
		}
		ch <- v
		close(ch)
		return ch, deferred, nil
	}
	if len(path) > 0 {
		slog.Debug("indexing pending future reader")
		r, err = r.Index(path...)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to index nested future reader: %w", err)
		}
	}
	return ch, func(ctx context.Context) error {
		if len(path) > 0 {
			defer closeNested(r)
		}
		slog.Debug("reading pending future")
		v, deferred, err := f(r)
		if err != nil {
			return fmt.Errorf("failed to read pending future: %w", err)
		}
		ch <- v
		close(ch)
		if deferred != nil {
			if err := deferred(ctx); err != nil {
				return err
			}
		}
		if r.Buffered() > 0 {
			return errors.New("received data after pending future value end")
		}
		return nil
	}, nil
}

// WriteFuture writes a `future` received from `v` to `w` using `f` to write its value
func WriteFuture[T any](v <-chan T, w ByteWriter, f func(T, ByteWriter) (func(context.Context, IndexWriter) error, error)) (func(context.Context, IndexWriter) error, error) {
	if v == nil {
		return nil, errors.New("outgoing future channel is nil")
	}
	slog.Debug("writing `future::pending` status byte")
	if err := w.WriteByte(0); err != nil {
		return nil, fmt.Errorf("failed to write `future::pending` byte: %w", err)
	}
	return func(ctx context.Context, w IndexWriter) error {
		slog.Debug("receiving outgoing pending future")
		var x T
		select {
		case v, ok := <-v:
			if !ok {
				return errors.New("outgoing pending future closed without a value")
			}
			x = v
		case <-ctx.Done():
			return fmt.Errorf("failed to receive outgoing pending future: %w", context.Cause(ctx))
		}
		slog.Debug("writing pending future value")
		var buf bytes.Buffer
		write, err := f(x, &buf)
		if err != nil {
			return fmt.Errorf("failed to encode pending future value: %w", err)
		}
		if err := w.WriteBytes(buf.Bytes()); err != nil {
			return fmt.Errorf("failed to write pending future value: %w", err)
		}
		if write != nil {
			return write(ctx, w)
		}
		return nil
	}, nil
}

// ReadFutureUnit reads a `future` without a payload from `r`.
// The returned channel receives a value once the future resolves and is closed afterwards.
// If the future is pending, the returned function waits for it to resolve and must be called.
// If reading fails, the channel never receives a value and is never closed.
func ReadFutureUnit(r IndexReadCloser, path ...uint32) (<-chan struct{}, func(context.Context) error, error) {
	slog.Debug("reading future status byte")
	ok, err := ReadFutureStatus(r)
	if err != nil {
		return nil, nil, err
	}
	ch := make(chan struct{}, 1)
	if ok {
		ch <- struct{}{}
		close(ch)
		return ch, nil, nil
	}
	if len(path) > 0 {
		slog.Debug("indexing pending future reader")
		r, err = r.Index(path...)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to index nested future reader: %w", err)
		}
	}
	return ch, func(context.Context) error {
		if len(path) > 0 {
			defer closeNested(r)
		}
		switch _, err := r.ReadByte(); err {
		case nil:
			return errors.New("received data for a `future` without a payload")
		case io.EOF:
			ch <- struct{}{}
			close(ch)
			return nil
		default:
			return fmt.Errorf("failed to wait for `future` path to close: %w", err)
		}
	}, nil
}

// WriteFutureUnit writes a `future` without a payload received from `v` to `w`
func WriteFutureUnit(v <-chan struct{}, w ByteWriter) (func(context.Context, IndexWriter) error, error) {
	if v == nil {
		return nil, errors.New("outgoing future channel is nil")
	}
	select {
	case _, ok := <-v:
		if !ok {
			return nil, errors.New("outgoing future closed without a value")
		}
		slog.Debug("writing `future::ready` status byte")
		if err := w.WriteByte(1); err != nil {
			return nil, fmt.Errorf("failed to write `future::ready` byte: %w", err)
		}
		return nil, nil
	default:
	}
	slog.Debug("writing `future::pending` status byte")
	if err := w.WriteByte(0); err != nil {
		return nil, fmt.Errorf("failed to write `future::pending` byte: %w", err)
	}
	return func(ctx context.Context, w IndexWriter) error {
		slog.Debug("receiving outgoing pending future")
		select {
		case _, ok := <-v:
			if !ok {
				return errors.New("outgoing pending future closed without a value")
			}
		case <-ctx.Done():
			return fmt.Errorf("failed to receive outgoing pending future: %w", context.Cause(ctx))
		}
		slog.Debug("closing pending future path")
		return w.ClosePath()
	}, nil
}
