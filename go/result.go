package wrpc

import (
	"errors"
	"fmt"
	"log/slog"
)

type Result[Ok, Err any] struct {
	Ok  *Ok
	Err *Err
}

func Ok[Err, Ok any](v Ok) *Result[Ok, Err] {
	return &Result[Ok, Err]{Ok: &v}
}

func Err[Ok, Err any](v Err) *Result[Ok, Err] {
	return &Result[Ok, Err]{Err: &v}
}

// ReadResultStatus reads a single byte from `r` and returns:
// - `true` for `result::ok`
// - `false` for `result::err`
func ReadResultStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `result` status byte: %w", err)
	}
	switch status {
	case 0:
		return true, nil
	case 1:
		return false, nil
	default:
		return false, fmt.Errorf("invalid `result` status byte %d", status)
	}
}

// ReadResult reads a single byte from `r`
func ReadResult[T, U any](r ByteReader, fOk func(ByteReader) (T, error), fErr func(ByteReader) (U, error)) (*Result[T, U], error) {
	ok, err := ReadResultStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		v, err := fErr(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read `result::err` value: %w", err)
		}
		return &Result[T, U]{Err: &v}, nil
	}
	v, err := fOk(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `result::ok` value: %w", err)
	}
	return &Result[T, U]{Ok: &v}, nil
}

func (v *Result[Ok, Err]) WriteTo(w ByteWriter, fOk func(*Ok, ByteWriter) error, fErr func(*Err, ByteWriter) error) error {
	switch {
	case v.Ok == nil && v.Err == nil:
		return errors.New("both result variants cannot be nil")
	case v.Ok != nil && v.Err != nil:
		return errors.New("exactly one result variant must non-nil")
	case v.Ok != nil:
		slog.Debug("writing `result::ok` status byte")
		if err := w.WriteByte(0); err != nil {
			return fmt.Errorf("failed to write `result::ok` status byte: %w", err)
		}
		slog.Debug("writing `result::ok` payload")
		if err := fOk(v.Ok, w); err != nil {
			return fmt.Errorf("failed to write `result::ok` payload: %w", err)
		}
		return nil
	default:
		slog.Debug("writing `result::err` status byte")
		if err := w.WriteByte(1); err != nil {
			return fmt.Errorf("failed to write `result::err` status byte: %w", err)
		}
		slog.Debug("writing `result::err` payload")
		if err := fErr(v.Err, w); err != nil {
			return fmt.Errorf("failed to write `result::err` payload: %w", err)
		}
		return nil
	}
}
