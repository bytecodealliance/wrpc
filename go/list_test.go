package wrpc_test

import (
	"bytes"
	"errors"
	"io"
	"math"
	"testing"

	wrpc "wrpc.io/go"
)

func TestReadBytes(t *testing.T) {
	want := bytes.Repeat([]byte{0x42}, 3<<20+1)
	got, err := wrpc.ReadBytes(bytes.NewReader(want), uint32(len(want)))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatal("read bytes do not match")
	}

	if _, err := wrpc.ReadBytes(bytes.NewReader(want[:10]), 11); !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("expected %v, got %v", io.ErrUnexpectedEOF, err)
	}
	if _, err := wrpc.ReadBytes(bytes.NewReader(nil), 1); !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("expected %v, got %v", io.ErrUnexpectedEOF, err)
	}
}

func TestHostileLength(t *testing.T) {
	if v := wrpc.NewSlice[[]uint64](math.MaxUint32); len(v) != 0 || cap(v) > 1<<20 {
		t.Fatalf("unexpected slice length %d and capacity %d", len(v), cap(v))
	}
	if _, err := wrpc.ReadBytes(bytes.NewReader(nil), math.MaxUint32); !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("expected %v, got %v", io.ErrUnexpectedEOF, err)
	}
}
