package wrpc

import (
	"fmt"
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

// ReadFuture reads a future from `r`
func ReadFuture[T any](r IndexReadCloser, f func(IndexReadCloser) (T, error), path ...uint32) (Receiver[T], error) {
	slog.Debug("reading future status byte")
	ok, err := ReadFutureStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		slog.Debug("indexing pending future reader")
		r, err := r.Index(append(path, 0)...)
		if err != nil {
			return nil, fmt.Errorf("failed to get future reader: %w", err)
		}
		return NewDecodeReceiver(r, f), nil
	}
	slog.Debug("reading ready future")
	v, err := f(r)
	if err != nil {
		return nil, err
	}
	return NewCompleteReceiver(v), nil
}
