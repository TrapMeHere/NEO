struct Args {
    data: array<u32, 8>,
}

@group(0) @binding(0)
var<storage, read_write> args: Args;

@compute @workgroup_size(1)
fn cs_main() {
    // DrawIndexedIndirect: word 1 is instance_count. 0 drops the draw.
    args.data[1] = 0u;
}
