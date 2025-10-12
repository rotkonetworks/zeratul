using BinaryFields
include("src/Ligerito.jl")
using .Ligerito

function test_verify_ligero_detailed()
    println("=== julia verify_ligero detailed test ===")

    # use exact same values as rust test
    challenges = [
        BinaryElem128(7272365410437037074),
        BinaryElem128(8045928772338086533)
    ]

    println("challenges: ", challenges)

    # test lagrange basis computation
    gr = evaluate_lagrange_basis(challenges)
    println("lagrange basis gr: ", gr)
    println("gr length: ", length(gr))

    # simulate opened row from rust (query 1, row length 4)
    # rust showed: [BinaryElem128(BinaryPoly128(3080729410)), ...]
    opened_row = [
        BinaryElem128(3080729410),
        BinaryElem128(3080730434),
        BinaryElem128(3080731458),
        BinaryElem128(3080732482)
    ]

    println("opened_row: ", opened_row)

    # compute dot = row' * gr (julia matrix multiplication)
    dot = opened_row' * gr
    println("dot = row' * gr = ", dot)

    # simulate yr values from rust
    yr = [
        BinaryElem128(61735827901135774182986907216971133455),
        BinaryElem128(302914484952793783354052168011858309327),
        BinaryElem128(208264898857241078353060933137547145100),
        BinaryElem128(162793167523517965603436417733875640477),
        BinaryElem128(2953452483698939371504693537323422011),
        BinaryElem128(73939633116789806150978656694284213559),
    ]

    # extend to length 64 with zeros for testing
    while length(yr) < 64
        push!(yr, BinaryElem128(0))
    end

    println("yr length: ", length(yr))
    println("yr[1:5]: ", yr[1:5])

    # multilinear basis computation for query 1
    n = Int(log2(length(yr)))  # should be 6 for 64 elements
    println("n = ", n)

    sks_vks = eval_sk_at_vks(2^n, BinaryElem128)
    println("sks_vks length: ", length(sks_vks))
    println("sks_vks[1:5]: ", sks_vks[1:5])

    # julia uses T(query - 1) for 1-based conversion
    # for query 1 (rust 0-based), julia would use qf = T(1 - 1) = T(0)
    query = 1  # rust query 0 -> julia query 1
    qf = BinaryElem128(query - 1)  # = BinaryElem128(0)
    println("query = ", query, ", qf = ", qf)

    # compute multilinear basis
    local_basis = zeros(BinaryElem128, 2^n)
    local_sks_x = Vector{BinaryElem128}(undef, length(sks_vks))

    evaluate_scaled_basis_inplace!(local_sks_x, local_basis, sks_vks, qf, BinaryElem128(1))

    # check which basis elements are non-zero
    non_zero_count = count(x -> x != BinaryElem128(0), local_basis)
    println("non_zero_count in local_basis: ", non_zero_count)

    if non_zero_count <= 10
        for (i, val) in enumerate(local_basis)
            if val != BinaryElem128(0)
                println("  local_basis[$i] = $val")
            end
        end
    end

    # compute e = yr' * local_basis
    e = yr' * local_basis
    println("e = yr' * local_basis = ", e)

    println("\n=== comparison ===")
    println("dot = ", dot)
    println("e = ", e)
    println("equal? ", e == dot)

    if e != dot
        println("MISMATCH in julia too!")
        println("difference: ", abs(e - dot))
    else
        println("SUCCESS in julia - values match!")
    end

    return e, dot
end

# run the test
test_verify_ligero_detailed()