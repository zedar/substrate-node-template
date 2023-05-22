#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

// dev_mode relax some restrictions placed on production pallets, such as no need to specify a
// weight on every `#[pallet::call]` Note: remove dev_mode before deploying in a production runtime.
#[frame_support::pallet(dev_mode)]
pub mod pallet {
	use frame_support::{
		pallet_prelude::*,
		sp_std,
		traits::{Currency, Randomness},
	};
	use frame_system::pallet_prelude::*;
	use sp_std::prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	// Defines generic data types that the pallet uses
	// By including the Currency and Randomness interfaces the pallet will be able to:
	// 	- access and manipulate user accounts and balances
	// 	- generate on-chain randomness
	// 	- set the limit on the number of newsletters a single user can subscribe to
	// frame_system|frame_support types: AccountId, BlockNumber, Hash
	#[pallet::config]
	pub trait Config: frame_system::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		type Currency: Currency<Self::AccountId>;

		// mapping of a feed hash to the block number
		type FeedRandomness: Randomness<Self::Hash, Self::BlockNumber>;

		// max number of created feeds. Each feed may have multiple subscriptions.
		#[pallet::constant]
		type MaxFeeds: Get<u32>;

		// max number of subscriptions per account
		#[pallet::constant]
		type MaxSubscriptions: Get<u32>;

		// max length of the subscription registration url
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

	type FeedId = [u8; 16];

	// TypeInfo macro forces to parse the underlying object into some JSON type. The Feed type is
	// generic over T, we don't want to include it in the TypeInfo generation step. That's why
	// skip_type_params(T) is needed
	#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	pub struct Feed<T: Config> {
		// unique identifier of the subscription
		pub unique_id: FeedId,
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

	// Stores single value that is a number of  available feeds.
	// The value is incremented each time new feed is created.
	// ValueQuery - if there is no value, the zero value is returned,
	// OptionQuery - None is returned,
	// ResultQuery - Err is returned.
	#[pallet::storage]
	pub(super) type FeedCount<T: Config> = StorageValue<_, u32, ValueQuery>;

	// Stores a map of feeds (each with unique_id) to their properties
	// The Twox64Concat is hashing algorithm used to store the map value.
	#[pallet::storage]
	pub(super) type Feeds<T: Config> = StorageMap<_, Twox64Concat, FeedId, Feed<T>>;

	// Maps user accounts to the subscribed feeds
	#[pallet::storage]
	pub(super) type Subscriptions<T: Config> =
		StorageMap<_, Twox64Concat, T::AccountId, BoundedVec<FeedId, T::MaxSubscriptions>>;

	// create_subscription creates new unique subscription
	// - create unique id
	// - ensure that total number of subscriptions does not exceed the maximum allowed

	#[pallet::error]
	pub enum Error<T> {
		// each feed must have a unique identifier
		DuplicatedFeed,
		// the total supply of feeds can't exceed `Config::MaxFeeds`
		FeedsOverflow,
		// a feed url can't exceed `Config::MaxRegistrationUrlLength`
		RegistrationUrlTooLong,
		// an account must subscribe only once to the feed
		DuplicatedSubscription,
		// an account can't exceed `Config::MaxSubscriptions`
		MaxSubscriptionsExceeded,
	}

	// generate_deposit generates a helper function on Pallet that handles event depositing/sending
	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		// runtime sends FeedCreated event when new feed has been successfully created
		FeedCreated { feed: FeedId, owner: T::AccountId },
	}

	// Callable functions
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		// Create a new/unique feed.
		#[pallet::weight(0)]
		pub fn create_feed(
			origin: OriginFor<T>,
			payment: PaymentType,
			url: Vec<u8>,
		) -> DispatchResult {
			// Make sure the caller is from the signed origin
			let sender = ensure_signed(origin)?;

			// Generate unique id for new feed
			let feed_unique_id = Self::new_unique_id();

			// Min new feed
			Self::mint_feed(&sender, feed_unique_id, payment, url.as_slice())?;

			Ok(())
		}
	}

	// Pallet internal functions
	impl<T: Config> Pallet<T> {
		// generate unique id for a new feed
		fn new_unique_id() -> FeedId {
			// create randomness
			let random = T::FeedRandomness::random(&b"unique_id"[..]).0;
			// randomness payload enables that multiple feeds can be created in the same block
			let unique_payload = (
				random,
				frame_system::Pallet::<T>::extrinsic_index().unwrap_or_default(),
				frame_system::Pallet::<T>::block_number(),
			);
			let encoded_payload = unique_payload.encode();
			frame_support::Hashable::blake2_128(&encoded_payload)
		}

		// Mint/create new feed
		pub fn mint_feed(
			owner: &T::AccountId,
			unique_id: FeedId,
			payment: PaymentType,
			url: &[u8],
		) -> Result<FeedId, DispatchError> {
			// create new feed
			let feed = Feed::<T> {
				unique_id,
				owner: owner.clone(),
				fee: None,
				payment_type: payment,
				registration_url: BoundedVec::<u8, T::MaxRegistrationUrlLength>::try_from(
					url.to_vec(),
				)
				.map_err(|_| Error::<T>::RegistrationUrlTooLong)?,
			};

			// check that feed does not exist in the storage map
			ensure!(!Feeds::<T>::contains_key(&feed.unique_id), Error::<T>::DuplicatedFeed);

			// check that a new feed does not exceed max available feeds
			let count = FeedCount::<T>::get();
			let new_count = count.checked_add(1).ok_or(Error::<T>::FeedsOverflow)?;
			ensure!(new_count <= T::MaxFeeds::get(), Error::<T>::FeedsOverflow);

			// write new feed to the storage and update the count
			Feeds::<T>::insert(feed.unique_id, feed);
			FeedCount::<T>::put(new_count);

			// deposit the event
			Self::deposit_event(Event::<T>::FeedCreated { feed: unique_id, owner: owner.clone() });

			Ok(unique_id)
		}
	}
}
