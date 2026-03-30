# Grace Period & Late Fee Implementation

## Overview

This document describes the grace period and late fee mechanism implemented in the Lending Contract. These features provide borrowers with a grace period after the loan due date before late fees accrue, and establish a penalty fee structure for loans that exceed their due date.

## Key Features

### 1. Grace Period

A grace period is a specified duration (default: 3 days) that begins immediately after a loan's due date. During this grace period:
- No late fees are charged
- Borrowed loans can still repay without penalty
- However, liquidation is blocked during the grace period (even if health factor is poor)

**Default Grace Period**: 259,200 seconds (3 days)

### 2. Late Fees

After the grace period expires, late fees begin to accrue on the outstanding loan balance:
- Calculated as: `principal × late_fee_rate × days_overdue / 10000`
- Applied daily after grace period expiration
- Entirely collected as protocol reserve (retained_yield)
- Late fees are included in the total repayment amount

**Default Late Fee Rate**: 500 basis points per day (5% per day)

### 3. Data Structures

#### Updated PoolState
```rust
pub struct PoolState {
    // ... existing fields ...
    pub grace_period_seconds: u64,    // Grace period duration
    pub late_fee_rate_bps: u32,       // Late fee rate in basis points per day
}
```

#### New Event: LateFeeChargedEvent
```rust
pub struct LateFeeChargedEvent {
    pub loan_id: u64,
    pub borrower: Address,
    pub late_fee: u64,
    pub days_overdue: u64,
    pub total_with_late_fees: u64,
    pub timestamp: u64,
}
```

#### New Storage Key
```rust
DataKey::LateFeesAccrued(u64)  // Per-loan late fee tracking
```

## API Functions

### Admin Functions

#### `set_grace_period(env, admin, grace_period_seconds)`
- **Access**: Admin only
- **Purpose**: Set the grace period duration for all future loans
- **Parameters**:
  - `grace_period_seconds`: New grace period in seconds
- **Returns**: Result<(), LendingError>

#### `set_late_fee_rate(env, admin, late_fee_rate_bps)`
- **Access**: Admin only
- **Purpose**: Set the late fee rate for all future loans
- **Parameters**:
  - `late_fee_rate_bps`: New rate in basis points per day
- **Returns**: Result<(), LendingError>

### Reader Functions

#### `get_grace_period(env)`
- **Purpose**: Get the current grace period duration
- **Returns**: u64 (grace period in seconds)

#### `get_late_fee_rate(env)`
- **Purpose**: Get the current late fee rate
- **Returns**: u32 (rate in basis points per day)

#### `is_in_grace_period(env, borrower)`
- **Purpose**: Check if a loan is still in its grace period
- **Parameters**:
  - `borrower`: Borrower address
- **Returns**: Result<bool, LendingError>
- **Logic**:
  - Returns true if: current_time <= due_date + grace_period
  - Returns false if: current_time > due_date + grace_period

#### `calculate_late_fee(env, borrower)`
- **Purpose**: Calculate accumulated late fees for a loan
- **Parameters**:
  - `borrower`: Borrower address
- **Returns**: Result<u64, LendingError>
- **Logic**:
  - If in grace period: returns 0
  - If after grace period: returns principal × rate × days_overdue / 10000
  - Daily rate calculation: rate_bps / 10000 / 365

#### `get_total_due_with_late_fees(env, borrower)`
- **Purpose**: Get total repayment amount including principal, interest, and late fees
- **Parameters**:
  - `borrower`: Borrower address
- **Returns**: Result<u64, LendingError>
- **Formula**: principal + interest + late_fees

#### `get_repayment_amount(env, borrower)` - **UPDATED**
- **Previous**: Returned principal + interest
- **Updated**: Now returns principal + interest + late_fees
- **Purpose**: Get actual amount required to repay the loan

### Liquidation Changes

#### `liquidate(env, liquidator, borrower, amount)` - **UPDATED**
- **New Check**: Liquidation is blocked if the loan is in its grace period
- **Returns**: `LendingError::InvalidAmount` if grace period is active
- **Purpose**: Prevent liquidation of loans that are within the grace period
- **Rationale**: Fairness to borrowers; allows time to repay before collateral is at risk

### Repayment Changes

#### `repay(env, borrower)` - **UPDATED**
- **Late Fee Collection**: Now collects late fees as part of repayment
- **Late Fee Distribution**:
  - Late fees go entirely to `pool.retained_yield` (protocol reserve)
  - Interest continues to follow normal distribution (90% to pool, 10% to protocol split between yield and bad debt reserve)
- **Event Emission**: Emits LateFeeChargedEvent if late fees were incurred
- **Cleanup**: Removes `LateFeesAccrued` storage entry for the loan

## Workflow Example

### Scenario: Loan with Grace Period and Late Fees

```
Time 0:      Borrow 1000 USDC, due in 1 day, grace period = 3 days, late fee = 5%
Time 1 day:  Due date reached, grace period starts (until time + 3 days)
Time 3 days: Still in grace period, no late fees
Time 4 days: Grace period expired (1 day after grace period end)
             - Late fees start accruing immediately
             - Amount overdue = 1000 USDC × 5% × 1 day ÷ 10000 = 0 (less than 1 wei precision)
Time 5 days: 2 days overdue
             - Late fee = 1000 × 500 × 2 ÷ 10000 = 100 USDC
Time 6 days: Borrower repays
             - Repay amount = 1000 (principal) + 50 (interest for 5 days) + 150 (late fees) = 1200
```

## Late Fee Calculation Details

### Formula
```
late_fee = principal × late_fee_rate_bps × days_overdue / 10000
```

Where:
- `principal`: Original loan principal
- `late_fee_rate_bps`: Rate in basis points per day (e.g., 500 = 5% per day)
- `days_overdue`: Full calendar days past grace period expiration
- Days are calculated as: `(current_time - grace_period_end) / (24 * 60 * 60)`

### Example Calculations
- Principal: 10,000 USDC
- Late Fee Rate: 500 bps (5% per day)
- Days Overdue: 2
- **Late Fee** = 10,000 × 500 × 2 / 10,000 = 1,000 USDC

## Events Emitted

### LateFeeChargedEvent
Emitted when repaying a loan with late fees:
```rust
LateFeeChargedEvent {
    loan_id: u64,           // Unique loan identifier
    borrower: Address,      // Borrower address
    late_fee: u64,          // Late fee amount charged
    days_overdue: u64,      // Number of days overdue
    total_with_late_fees: u64,  // Total repayment including late fees
    timestamp: u64,         // Block timestamp
}
```

## Testing

The implementation includes comprehensive tests covering:

1. **Grace Period Defaults**
   - Verify default grace period (3 days)
   - Verify default late fee rate (5% per day)

2. **Admin Functions**
   - Set grace period (admin only)
   - Set late fee rate (admin only)
   - Non-admin rejection

3. **Grace Period Logic**
   - No late fees during grace period
   - Late fees after grace period expires
   - Grace period expiration detection

4. **Late Fee Calculation**
   - Correct calculation after grace period
   - Per-day accumulation
   - Multiple loans with different grace periods

5. **Liquidation Protection**
   - Liquidation blocked during grace period
   - Liquidation allowed after grace period

6. **Repayment Integration**
   - Late fees collected on repayment
   - Late fees go to protocol reserve
   - Events emitted correctly

7. **Multi-Loan Scenarios**
   - Multiple loans with different due dates
   - Each loan has independent grace period
   - Correct late fee tracking per loan

## State Management

### Persistent Storage Keys
- `LateFeesAccrued(u64)`: Tracks accumulated late fees per loan_id
- Cleared when loan is repaid or liquidated

### Instance Storage (PoolState)
- `grace_period_seconds`: Global setting for all loans
- `late_fee_rate_bps`: Global setting for all loans

## Security Considerations

1. **Reentrancy**: All functions use reentrancy guard (enter/exit)
2. **Authorization**: Admin functions require auth and admin verification
3. **Overflow Prevention**: All arithmetic uses checked operations with saturation
4. **Grace Period Protection**: Prevents unexpected liquidations during grace period

## Revenue Model

Late fees are designed to:
1. **Compensate** for delayed repayment risk
2. **Incentivize** timely repayment
3. **Build reserves** for protocol maintenance
4. **Protect** lenders through accumulating reserves

All late fees go to `pool.retained_yield`, which can be withdrawn using `withdraw_priority()`.

## Future Enhancements

Potential improvements:
1. Variable late fee rates based on health factor
2. Late fee amnesty for specific circumstances
3. Late fee waivers after successful repayment history
4. Progressive penalty structure (increasing fees over time)
5. Integration with liquidation pricing (e.g., discount increases with delay)

## Compatibility Notes

- **Backward Compatible**: Existing loans unaffected; grace period/late fee logic is independent
- **No Breaking Changes**: All existing functions continue to work as before
- **Additive Only**: New features don't modify existing data structures (only extend PoolState)
