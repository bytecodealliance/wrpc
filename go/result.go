package wrpc

// Result is a `result<Ok, Err>` value, exactly one of its fields is set
type Result[Ok, Err any] struct {
	Ok  *Ok
	Err *Err
}

// Ok returns a `result::ok` with value `v`
func Ok[Err, Ok any](v Ok) *Result[Ok, Err] {
	return &Result[Ok, Err]{Ok: &v}
}

// Err returns a `result::err` with value `v`
func Err[Ok, Err any](v Err) *Result[Ok, Err] {
	return &Result[Ok, Err]{Err: &v}
}
