package wrpc_test

import (
	"bytes"
	"errors"
	"io"
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
