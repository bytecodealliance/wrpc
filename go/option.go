package wrpc

import (
	"fmt"
	"log/slog"
)

func FlattenOption[T any](v **T) *T {
	if v == nil {
		return nil
	} else {
		return *v
	}
}

// ReadOptionStatus reads a single byte from `r` and returns:
// - `true` for `option::some`
// - `false` for `option::none`
func ReadOptionStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `option` status byte: %w", err)
	}
	switch status {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, fmt.Errorf("invalid `option` status byte %d", status)
	}
}

// ReadOption reads an option from `r`
func ReadOption[T any](r ByteReader, f func(ByteReader) (T, error)) (*T, error) {
	slog.Debug("reading option status byte")
	ok, err := ReadOptionStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		return nil, nil
	}
	slog.Debug("reading `option::some` payload")
	v, err := f(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `option::some` value: %w", err)
	}
	return &v, nil
}

// ReadFlatOption reads an option from `r` without pointer indirection
func ReadFlatOption[T any](r IndexReader, f func(IndexReader) (T, error)) (v T, err error) {
	slog.Debug("reading option status byte")
	ok, err := ReadOptionStatus(r)
	if err != nil {
		return v, err
	}
	if !ok {
		return v, err
	}
	slog.Debug("reading `option::some` payload")
	v, err = f(r)
	if err != nil {
		return v, fmt.Errorf("failed to read `option::some` value: %w", err)
	}
	return v, nil
}

func WriteOption[T any](v *T, w ByteWriter, f func(T, ByteWriter) error) error {
	if v == nil {
		slog.Debug("writing `option::none` status byte")
		if err := w.WriteByte(0); err != nil {
			return fmt.Errorf("failed to write `option::none` byte: %w", err)
		}
		return nil
	}
	slog.Debug("writing `option::some` status byte")
	if err := w.WriteByte(1); err != nil {
		return fmt.Errorf("failed to write `option::some` status byte: %w", err)
	}
	slog.Debug("writing `option::some` payload")
	if err := f(*v, w); err != nil {
		return fmt.Errorf("failed to write `option::some` payload: %w", err)
	}
	return nil
}
