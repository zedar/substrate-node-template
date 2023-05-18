#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

// dev_mode relax some restrictions placed on production pallets, such as no need to specify a weight on every `#[pallet::call]`
// Note: remove dev_mode before deploying in a production runtime.
#[frame_support::pallet(dev_mode)]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_support::traits::{Currency, Randomness};
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	// Defines generic data types that the pallet uses
	// By including the Currency and Randomness interfaces the pallet will be able to:
	//	- access and manipulate user accounts and balances
	//	- generate on-chain randomness
	// 	- set the limit on the number of newsletters a single user can subscribe to
	// frame_system|frame_support types: AccountId, BlockNumber, Hash
	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Currency: Currency<Self::AccountId>;
		// mapping of a subscription hash to the block number
		type SubscriptionRandomness: Randomness<Self::Hash, Self::BlockNumber>;

		#[pallet::constant]
		type MaxSubscriptions: Get<u32>;

		#[pallet::constant]
		type MaxRegistrationUrlLength: Get<u32>;
	}

	// Payment type that can be either one time or recurrent
	#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	pub enum PaymentType {
		OneTime,
		Recurrent,
	}

	type BalanceOf<T> =
		<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

	type SubscriptionId = [u8; 16];

	// TypeInfo macro forces to parse the underlying object into some JSON type. The Subscription type is generic over T,
	// we don't want to include it in the TypeInfo generation step. That's why skip_type_params(T) is needed
	#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	pub struct Subscription<T: Config> {
		// unique identifier of the subscription
		pub unique_id: SubscriptionId,
		// owner of the subscription
		pub owner: T::AccountId,
		// price for the subscription
		pub fee: Option<BalanceOf<T>>,
		// payment type
		pub payment_type: PaymentType,
		// url of where to register/revoke a subscription
		// Vec<u8> represents a string in substrate
		// pub registration_url: Vec<u8>,
		pub registration_url: BoundedVec<u8, T::MaxRegistrationUrlLength>,
	}

	// Stores single value that is a number of  available subscriptions.
	// The value is incremented each time new subscription is created.
	// ValueQuery - if there is no value, the zero value is returned,
	// OptionQuery - None is returned,
	// ResultQuery - Err is returned.
	#[pallet::storage]
	pub(super) type SubscriptionsCount<T: Config> = StorageValue<_, u64, ValueQuery>;

	// Stores a map of subscription (its unique_id) to its properties
	// The Twox64Concat is hashing algorithm used to store the map value.
	#[pallet::storage]
	pub(super) type SubscriptionMap<T: Config> =
		StorageMap<_, Twox64Concat, SubscriptionId, Subscription<T>>;

	// Maps user accounts to the subscriptions they are subscribed in.
	#[pallet::storage]
	pub(super) type ConsumerOfSubscriptions<T: Config> =
		StorageMap<_, Twox64Concat, T::AccountId, BoundedVec<SubscriptionId, T::MaxSubscriptions>>;
}
