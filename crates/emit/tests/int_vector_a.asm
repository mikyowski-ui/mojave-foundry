.procedure main
.procedure sumxy args=2
.literal greeting "hi"
.entry init
init:
    global_area
    push.i 0
    push.i 0
    critical_off
    push.a main
    jmp
main:
    frame
    push.a after
    v_to_r
    push.i 5
    push.i 7
    push.i 2
    push.p sumxy
    call
after:
    push.a else
    swap
    push.i 10
    gt
    bz
    push.i 99
    push.i 0
    store_global
    push.a join
    jmp
else:
    push.i -1
    push.i 0
    store_global
join:
    drop_frame
    restore_base
    ret
sumxy:
    frame
    push.i 0
    fetch_local
    push.i 1
    fetch_local
    add
    v_to_r
    drop_frame
    r_to_v
    restore_base
    ret
